//! An alert card into a PNG: `cargo run -p pcm-render --example alert -- OUT.png`

use pcm_render::text::Fonts;
use pcm_render::theme::Theme;
use pcm_render::widget::alert_card;

fn main() {
    let out = std::env::args().nth(1).unwrap_or_else(|| "alert.png".into());
    let mut fonts = Fonts::system();
    let body = "Занято 93% (20,3 из 21,8 ГБ). Больше всех: chrome — 3,4 ГБ.";
    let pm = alert_card(&mut fonts, 1.5, "Память почти закончилась", body, Theme::Heat.scale().at(1.0)).unwrap();
    pm.save_png(out).unwrap();
}
