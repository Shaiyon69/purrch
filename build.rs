fn main() {
    // Icon id 1: Explorer shows the exe with it and the tray loads it by that id.
    embed_resource::compile("purrch.rc", embed_resource::NONE).manifest_optional().unwrap();
}
