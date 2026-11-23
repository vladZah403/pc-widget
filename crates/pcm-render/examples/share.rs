//! The picture "Поделиться" makes of a session:
//! `cargo run -p pcm-render --example share -- OUT.png [FILE.pcms] [ru|en|uk|de] [light]`

use pcm_render::format::set_language;
use pcm_render::report::sample_session;
use pcm_render::share::share_card;
use pcm_render::text::Fonts;
use pcm_render::theme::Theme;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let out = args.get(1).cloned().unwrap_or_else(|| "share.png".into());
    let s = match args.get(2).filter(|a| a.ends_with(".pcms")) {
        Some(f) => pcm_core::session::Session::load(std::path::Path::new(f)).expect("read the session"),
        None => sample_session(),
    };
    set_language(args.get(3).map_or("ru", String::as_str));
    let light = args.iter().any(|a| a == "light");
    pcm_render::theme::set_light(light);
    let sc = Theme::Heat.scale(); // follows the light look
    let mut fonts = Fonts::system();
    share_card(&mut fonts, 2.0, &sc, &s).expect("render").save_png(&out).expect("png");
    println!("{out}");
}
