//! The pictures of the README and the website, in one language:
//! `cargo run -p pcm-render --example docs -- OUT_DIR [en|ru|uk|de]`
//! (the PC Monitor window's pages come from `PC-Monitor.exe --dashboard
//! --offscreen --screenshot`).

use pcm_render::format::{dec, set_language, tr, trf};
use pcm_render::overlay::{Look, preview, sample_game};
use pcm_render::text::Fonts;
use pcm_render::theme::{DARK_INK, LIGHT_INK, Theme};
use pcm_render::widget::{Renderer, View, WidgetLook, hover_card, sample_data, tray_icon_filled};
use tiny_skia::{Color, Pixmap, PixmapPaint, Transform};

const BG: (u8, u8, u8) = (17, 20, 27);
const GAP: u32 = 24;

/// The pictures side by side (top-aligned) on the page background.
fn row(pics: &[Pixmap], scale: f32) -> Pixmap {
    let gap = (GAP as f32 * scale) as u32;
    let w = pics.iter().map(|p| p.width() + gap).sum::<u32>() + gap;
    let h = pics.iter().map(|p| p.height()).max().unwrap() + 2 * gap;
    let mut sheet = Pixmap::new(w, h).unwrap();
    sheet.fill(Color::from_rgba8(BG.0, BG.1, BG.2, 255));
    let mut x = gap;
    for p in pics {
        sheet.draw_pixmap(
            x as i32,
            gap as i32,
            p.as_ref(),
            &PixmapPaint::default(),
            Transform::identity(),
            None,
        );
        x += p.width() + gap;
    }
    sheet
}

fn main() {
    let out = std::env::args().nth(1).unwrap_or_else(|| ".".into());
    set_language(&std::env::args().nth(2).unwrap_or_else(|| "en".into()));
    let s = 1.5; // sharp on high-DPI screens
    let mut fonts = Fonts::system();
    let save = |pm: &Pixmap, name: &str| {
        let path = format!("{out}/{name}.png");
        pm.save_png(&path).expect("png");
        println!("{path}: {}x{}", pm.width(), pm.height());
    };

    // the widget: cards dark and light, compact, and 1.8's classic look
    let d = sample_data();
    let mut r = Renderer::new(&mut fonts);
    let cards = View { look: WidgetLook::Cards, ..View::default() };
    // the website's top: one widget, twice the size (shown at half)
    save(&r.render(&d, &View { scale: 2.0, ..cards.clone() }).expect("render").pm, "widget-2x");
    let mut shot = |v: View| r.render(&d, &View { scale: s, ..v }).expect("render").pm;
    let dark = shot(cards.clone());
    let light = shot(View { light: true, ..cards.clone() });
    let compact = shot(View { compact: true, ..cards.clone() });
    save(&row(&[dark, light, compact], s), "widget");
    // the gauges look, dark and light
    let gauges = View { look: WidgetLook::Gauges, ..View::default() };
    let gd = shot(gauges.clone());
    let gl = shot(View { light: true, ..gauges });
    save(&row(&[gd, gl], s), "widget-gauges");
    let classic = shot(View::default());
    let classic_compact = shot(View { compact: true, ..View::default() });
    save(&row(&[classic, classic_compact], s), "widget-classic");

    // the FPS panel over a game: the card and the one-line look
    let sc = Theme::Heat.scale();
    let info = sample_game();
    let card = preview(&mut fonts, s, 100, &sc, &info, Look::Card, 0.85, "tl").unwrap();
    let line = preview(&mut fonts, s, 100, &sc, &info, Look::Line, 0.5, "br").unwrap();
    save(&row(&[card, line], s), "fps");

    // the tray: icons and the card shown when the mouse is over one
    let mut pics = Vec::new();
    for light in [false, true] {
        let sc = if light { Theme::Heat.scale_dark().on_light() } else { Theme::Heat.scale_dark() };
        let text = if light { LIGHT_INK.text } else { DARK_INK.text };
        let rows = vec![
            (tr("Нагрузка"), "12%".to_string(), sc.at(0.12)),
            (tr("Температура"), "42°C".to_string(), sc.heat(42.0)),
            (tr("Частота"), trf("{v} ГГц", &[("v", dec("3.84".to_string()))]), text),
            (tr("Больше всего"), format!("chrome · {}%", dec("1.6".to_string())), text),
        ];
        pics.push(
            hover_card(&mut fonts, s, &tr("Процессор"), &rows, &tr("Клик — открыть PC Monitor"), light)
                .unwrap(),
        );
    }
    let sc = Theme::Heat.scale_dark();
    let size = (32.0 * s) as u32;
    let mut icons = Pixmap::new(4 * (size + 8), size).unwrap();
    for (i, (t, v)) in [("12", 0.12), ("42", 0.42), ("97", 0.97), ("66°", 0.55)].iter().enumerate() {
        let icon = tray_icon_filled(&mut fonts, size, t, sc.at(*v)).unwrap();
        icons.draw_pixmap(
            (i as u32 * (size + 8)) as i32,
            0,
            icon.as_ref(),
            &PixmapPaint::default(),
            Transform::identity(),
            None,
        );
    }
    pics.push(icons);
    save(&row(&pics, s), "tray");

    // the classic widget at twice the size: its rows in three groups (the
    // hero cut off), six widths, the four palettes, and the taskbar panel
    let mut r = Renderer::new(&mut fonts);
    // the processor's name as a real one reads (the sample's is a placeholder)
    let d = pcm_render::widget::Data { cpu_name: "Ryzen 5 5500U".into(), ..d.clone() };
    const ALL: [&str; 13] = [
        "cores",
        "ram",
        "gpu",
        "disks",
        "temps",
        "battery",
        "ssd",
        "fans",
        "net",
        "net_graph",
        "ping",
        "disk_io",
        "uptime",
    ];
    let only =
        |keep: &[&str]| ALL.iter().filter(|k| !keep.contains(k)).map(|k| k.to_string()).collect::<Vec<_>>();
    for (name, keep) in [
        ("glance-load", &["cores", "ram", "gpu", "disks"][..]),
        ("glance-sensors", &["temps", "battery", "ssd"][..]),
        ("glance-network", &["net", "net_graph", "ping", "disk_io"][..]),
    ] {
        let pm =
            r.render(&d, &View { scale: 2.0, hidden: only(keep), ..View::default() }).expect("render").pm;
        save(&rows_only(&pm, 2.0), name);
    }
    for w in [264.0f32, 352.0, 440.0, 528.0, 660.0, 792.0] {
        let pm = r.render(&d, &View { scale: 2.0, width: w, ..View::default() }).expect("render").pm;
        save(&pm, &format!("shape-{}", w as u32));
    }
    for (name, theme) in [
        ("theme-heat", Theme::Heat),
        ("theme-neon", Theme::Neon),
        ("theme-ice", Theme::Ice),
        ("theme-custom", Theme::Custom((90, 120, 255), (255, 80, 160))),
    ] {
        let pm =
            r.render(&d, &View { scale: 2.0, compact: true, theme, ..View::default() }).expect("render").pm;
        save(&pm, name);
    }
    let bar =
        pcm_render::widget::taskbar_panel(&mut fonts, 2.0, &d, &Theme::Heat, false, 80, None).expect("panel");
    save(&bar, "taskbar");
}

/// The rows of a classic widget, without its top (the big number) and its
/// rounded frame: what the website shows of each group.
fn rows_only(pm: &Pixmap, scale: f32) -> Pixmap {
    let top = (pcm_render::widget::HERO_H * scale) as u32;
    let side = (8.0 * scale) as u32;
    let w = pm.width() - 2 * side;
    let h = pm.height().saturating_sub(top + side);
    let mut out = Pixmap::new(w, h).unwrap();
    out.draw_pixmap(
        -(side as i32),
        -(top as i32),
        pm.as_ref(),
        &PixmapPaint::default(),
        Transform::identity(),
        None,
    );
    out
}
