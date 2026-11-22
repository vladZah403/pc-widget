//! The cards look's hover card (dark and light) and its tray icons into PNGs:
//! `cargo run -p pcm-render --example hover -- OUT_DIR`

use pcm_render::text::Fonts;
use pcm_render::theme::{DARK_INK, LIGHT_INK, Theme};
use pcm_render::widget::{hover_card, tray_icon_filled};

fn main() {
    let out = std::env::args().nth(1).unwrap_or_else(|| ".".into());
    let mut fonts = Fonts::system();
    for light in [false, true] {
        let sc = if light { Theme::Heat.scale_dark().on_light() } else { Theme::Heat.scale_dark() };
        let text = if light { LIGHT_INK.text } else { DARK_INK.text };
        let rows = vec![
            ("Нагрузка".to_string(), "12%".to_string(), sc.at(0.12)),
            ("Температура".to_string(), "42°C".to_string(), sc.heat(42.0)),
            ("Частота".to_string(), "3,84 ГГц".to_string(), text),
            ("Больше всего".to_string(), "chrome · 1,6%".to_string(), text),
        ];
        let pm = hover_card(&mut fonts, 1.0, "Процессор", &rows, "Клик — открыть PC Monitor", light).unwrap();
        let name = if light { "hover-light" } else { "hover-dark" };
        pm.save_png(format!("{out}/{name}.png")).unwrap();
    }
    let sc = Theme::Heat.scale_dark();
    for (i, (t, v)) in [("12", 0.12), ("42", 0.42), ("97", 0.97), ("66°", 0.55)].iter().enumerate() {
        let pm = tray_icon_filled(&mut fonts, 32, t, sc.at(*v)).unwrap();
        pm.save_png(format!("{out}/tray-filled-{i}.png")).unwrap();
    }
}
