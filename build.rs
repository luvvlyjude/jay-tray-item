use wl_client_builder::Builder;

fn main() {
    println!("cargo:rerun-if-changed=wayland-protocols");
    Builder::default().with_mutable_data(true).build().unwrap();
}
