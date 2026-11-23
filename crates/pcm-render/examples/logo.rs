//! The logo files: `cargo run -p pcm-render --example logo -- ICO_DIR PNG_DIR`
//! writes ICO_DIR/app.ico, PNG_DIR/logo-256.png (the window's icon and the
//! website) and PNG_DIR/logo-64.png (the site's favicon).

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let ico_dir = args.first().map_or("assets", String::as_str);
    let png_dir = args.get(1).map_or("docs/img", String::as_str);
    let ico = pcm_render::logo::ico().expect("ico");
    std::fs::write(format!("{ico_dir}/app.ico"), ico).expect("write ico");
    for s in [256, 64] {
        pcm_render::logo::logo(s).expect("logo").save_png(format!("{png_dir}/logo-{s}.png")).expect("png");
    }
    println!("{ico_dir}/app.ico, {png_dir}/logo-256.png, {png_dir}/logo-64.png");
}
