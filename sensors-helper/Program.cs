// PC Monitor 2.0, stage 0: pcm-sensors.exe, the LibreHardwareMonitor helper.
//
// Reads every sensor LibreHardwareMonitorLib finds and prints one JSON line per
// reading to stdout. The parent (PC-Monitor.exe) talks to it only through the
// child process's stdin/stdout: no ports, no pipes anyone else can open.
//
//   pcm-sensors.exe [--interval ms] [--once] [--parts cpu,gpu,board,storage,battery]
//
// Default parts: cpu,gpu,board,storage. The battery is read by PC-Monitor.exe
// itself (IOCTL_BATTERY_*): through the library it costs ~70 ms a second.
// Drives are read every 30 s only (their SMART log and temperature change
// slowly; with admin rights a read costs ~17 ms of the ~39 ms of a whole
// reading); between those the last values go out again.
//
// Lines on stdout:
//   {"type":"hello","protocol":1,"lhm":"0.9.6.0","admin":false,"pid":123}
//   {"type":"sensors","seq":1,"read_ms":12.3,"mem_mb":31.2,"hw":[...]}
//   {"type":"error","message":"..."}
// Stdin: "quit" or end of input (the parent died) ends the helper.
//
// Written for the C# 5 compiler that ships with .NET Framework 4.8 (csc.exe in
// %WINDIR%\Microsoft.NET\Framework64\v4.0.30319), so no newer syntax.

using System;
using System.Diagnostics;
using System.Globalization;
using System.IO;
using System.Reflection;
using System.Runtime.CompilerServices;
using System.Security.Principal;
using System.Text;
using System.Threading;

namespace PcMonitor.Sensors
{
    static class Program
    {
        const int Protocol = 1;
        static readonly object WriteLock = new object();
        static volatile bool quit;
        static readonly ManualResetEvent QuitEvent = new ManualResetEvent(false);

        static int Main(string[] args)
        {
            int interval = 1000;
            bool once = false;
            string parts = "cpu,gpu,board,storage";
            for (int i = 0; i < args.Length; i++)
            {
                if (args[i] == "--once") once = true;
                else if (args[i] == "--parts" && i + 1 < args.Length) parts = args[++i];
                else if (args[i] == "--interval" && i + 1 < args.Length
                         && int.TryParse(args[++i], out interval) && interval >= 100) { }
                else
                {
                    Console.Error.WriteLine("usage: pcm-sensors.exe [--interval ms (>= 100)] [--once]"
                                            + " [--parts cpu,gpu,board,storage,battery]");
                    return 2;
                }
            }
            // LibreHardwareMonitorLib and its System.* DLLs sit next to the exe; load
            // them by name whatever version is asked for (no binding redirects needed)
            AppDomain.CurrentDomain.AssemblyResolve += ResolveNextToExe;
            Console.OutputEncoding = new UTF8Encoding(false);

            if (!once)
            {
                var watcher = new Thread(WatchStdin);
                watcher.IsBackground = true;
                watcher.Start();
            }
            try
            {
                return Run(interval, once, ("," + parts.ToLowerInvariant().Replace(" ", "") + ","));
            }
            catch (Exception e)
            {
                Error(e.GetType().Name + ": " + e.Message);
                return 1;
            }
        }

        static Assembly ResolveNextToExe(object sender, ResolveEventArgs e)
        {
            string name = new AssemblyName(e.Name).Name;
            string path = Path.Combine(AppDomain.CurrentDomain.BaseDirectory, name + ".dll");
            return File.Exists(path) ? Assembly.LoadFrom(path) : null;
        }

        static void WatchStdin()
        {
            try
            {
                string line;
                while ((line = Console.In.ReadLine()) != null)
                {
                    if (line.Trim() == "quit") break;
                }
            }
            catch (IOException) { }
            quit = true;
            QuitEvent.Set();
        }

        // Kept apart from Main so the library is loaded only after AssemblyResolve is set.
        [MethodImpl(MethodImplOptions.NoInlining)]
        static int Run(int interval, bool once, string parts)
        {
            var reader = new Reader(parts);
            try
            {
                Emit(new Json().Obj()
                    .Str("type", "hello").Num("protocol", Protocol)
                    .Str("lhm", reader.LibraryVersion).Bool("admin", IsAdmin())
                    .Num("pid", Process.GetCurrentProcess().Id).End().ToString());
                long seq = 0;
                while (!quit)
                {
                    var sw = Stopwatch.StartNew();
                    var json = new Json().Obj().Str("type", "sensors").Num("seq", ++seq);
                    string hw = reader.Read();
                    double ms = sw.Elapsed.TotalMilliseconds;
                    json.Num("read_ms", Math.Round(ms, 1)).Num("mem_mb", Math.Round(PrivateMb(), 1))
                        .Raw("hw", hw).End();
                    Emit(json.ToString());
                    if (once) break;
                    QuitEvent.WaitOne(Math.Max(50, interval - (int)ms));
                }
            }
            finally
            {
                reader.Close();
            }
            return 0;
        }

        static double PrivateMb()
        {
            using (var p = Process.GetCurrentProcess()) return p.PrivateMemorySize64 / 1048576.0;
        }

        static bool IsAdmin()
        {
            using (var id = WindowsIdentity.GetCurrent())
                return new WindowsPrincipal(id).IsInRole(WindowsBuiltInRole.Administrator);
        }

        static void Emit(string line)
        {
            lock (WriteLock)
            {
                Console.Out.Write(line);
                Console.Out.Write('\n');
                Console.Out.Flush();
            }
        }

        public static void Error(string message)
        {
            Emit(new Json().Obj().Str("type", "error").Str("message", message).End().ToString());
        }
    }

    class Reader
    {
        readonly LibreHardwareMonitor.Hardware.Computer computer;
        /// <summary>Drives are updated this seldom (ms).</summary>
        const long StorageEveryMs = 30000;
        readonly Stopwatch sinceStorage = new Stopwatch();
        bool storageDue = true;

        /// <param name="parts">",cpu,gpu,...," (lower case, comma on both ends)</param>
        public Reader(string parts)
        {
            computer = new LibreHardwareMonitor.Hardware.Computer();
            computer.IsCpuEnabled = parts.Contains(",cpu,");
            computer.IsGpuEnabled = parts.Contains(",gpu,");
            computer.IsMotherboardEnabled = parts.Contains(",board,");  // fans (admin rights)
            computer.IsStorageEnabled = parts.Contains(",storage,");    // SMART for SATA / Intel RST
            computer.IsBatteryEnabled = parts.Contains(",battery,");
            computer.Open();
        }

        public string LibraryVersion
        {
            get { return typeof(LibreHardwareMonitor.Hardware.Computer).Assembly.GetName().Version.ToString(); }
        }

        /// <summary>A JSON array of all hardware (sub-hardware flattened, with "parent").</summary>
        public string Read()
        {
            var arr = new StringBuilder("[");
            bool first = true;
            storageDue = !sinceStorage.IsRunning || sinceStorage.ElapsedMilliseconds >= StorageEveryMs;
            if (storageDue) sinceStorage.Restart();
            foreach (var hw in computer.Hardware) Add(arr, hw, null, ref first);
            return arr.Append(']').ToString();
        }

        void Add(StringBuilder arr, LibreHardwareMonitor.Hardware.IHardware hw, string parent, ref bool first)
        {
            string id = hw.Identifier.ToString();
            var sw = Stopwatch.StartNew();
            try
            {
                if (storageDue || hw.HardwareType != LibreHardwareMonitor.Hardware.HardwareType.Storage)
                    hw.Update();
            }
            catch (Exception e)
            {
                Program.Error("update " + id + ": " + e.Message);
                return;
            }
            var j = new Json().Obj().Str("id", id).Str("name", hw.Name).Str("kind", hw.HardwareType.ToString());
            if (parent != null) j.Str("parent", parent);
            j.Num("update_ms", Math.Round(sw.Elapsed.TotalMilliseconds, 1));
            var sensors = new StringBuilder("[");
            bool firstSensor = true;
            foreach (var s in hw.Sensors)
            {
                if (!s.Value.HasValue || float.IsNaN(s.Value.Value) || float.IsInfinity(s.Value.Value)) continue;
                if (!firstSensor) sensors.Append(',');
                firstSensor = false;
                sensors.Append(new Json().Obj().Str("id", s.Identifier.ToString()).Str("name", s.Name)
                    .Str("kind", s.SensorType.ToString()).Num("value", s.Value.Value).End().ToString());
            }
            j.Raw("sensors", sensors.Append(']').ToString()).End();
            if (!first) arr.Append(',');
            first = false;
            arr.Append(j.ToString());
            foreach (var sub in hw.SubHardware) Add(arr, sub, id, ref first);
        }

        public void Close()
        {
            try
            {
                computer.Close();
            }
            catch (Exception) { }
        }
    }

    /// <summary>Just enough JSON writing for flat objects; no dependencies.</summary>
    class Json
    {
        readonly StringBuilder sb = new StringBuilder();
        bool first = true;

        public Json Obj() { sb.Append('{'); return this; }
        public Json End() { sb.Append('}'); return this; }
        public override string ToString() { return sb.ToString(); }

        Json Key(string k)
        {
            if (!first) sb.Append(',');
            first = false;
            Quote(k);
            sb.Append(':');
            return this;
        }

        public Json Str(string k, string v) { Key(k); if (v == null) sb.Append("null"); else Quote(v); return this; }
        public Json Bool(string k, bool v) { Key(k); sb.Append(v ? "true" : "false"); return this; }
        public Json Raw(string k, string json) { Key(k); sb.Append(json); return this; }

        public Json Num(string k, double v)
        {
            Key(k);
            if (double.IsNaN(v) || double.IsInfinity(v)) sb.Append("null");
            else sb.Append(v.ToString("0.###", CultureInfo.InvariantCulture));
            return this;
        }

        void Quote(string s)
        {
            sb.Append('"');
            foreach (char c in s)
            {
                switch (c)
                {
                    case '"': sb.Append("\\\""); break;
                    case '\\': sb.Append("\\\\"); break;
                    case '\n': sb.Append("\\n"); break;
                    case '\r': sb.Append("\\r"); break;
                    case '\t': sb.Append("\\t"); break;
                    default:
                        if (c < 0x20) sb.Append("\\u").Append(((int)c).ToString("x4"));
                        else sb.Append(c);
                        break;
                }
            }
            sb.Append('"');
        }
    }
}
