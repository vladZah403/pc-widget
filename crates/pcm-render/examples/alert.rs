//! An alert card into a PNG: `cargo run -p pcm-render --example alert -- OUT.png [TITLE BODY]`

use pcm_render::text::Fonts;
use pcm_render::theme::Theme;
use pcm_render::widget::alert_card;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let out = args.get(1).cloned().unwrap_or_else(|| "alert.png".into());
    let title = args.get(2).map_or("Память почти закончилась", String::as_str);
    let body =
        args.get(3).map_or("Занято 93% (20,3 из 21,8 ГБ). Больше всех: chrome — 3,4 ГБ.", String::as_str);
    let mut fonts = Fonts::system();
    let pm = alert_card(&mut fonts, 1.5, title, body, Theme::Heat.scale().at(1.0)).unwrap();
    pm.save_png(out).unwrap();
}
