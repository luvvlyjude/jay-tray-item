#![deny(unsafe_code)]

// Generated bindings: wl-client-builder emits unsafe calls and leaves bitfield `Iter` structs
// without an allow.
#[allow(dead_code, unsafe_code)]
mod protocols {
    include!(concat!(env!("OUT_DIR"), "/wayland-protocols/mod.rs"));
}

use protocols::{
    jay_tray_v1::{
        jay_tray_item_v1::{JayTrayItemV1, JayTrayItemV1EventHandler, JayTrayItemV1Ref},
        jay_tray_v1::JayTrayV1,
    },
    wayland::{
        wl_buffer::WlBuffer,
        wl_callback::{WlCallbackEventHandler, WlCallbackRef},
        wl_compositor::WlCompositor,
        wl_display::WlDisplay,
        wl_output::{WlOutput, WlOutputEventHandler, WlOutputRef},
        wl_pointer::{WlPointer, WlPointerButtonState, WlPointerEventHandler, WlPointerRef},
        wl_registry::{WlRegistry, WlRegistryEventHandler, WlRegistryRef},
        wl_seat::{WlSeat, WlSeatCapability, WlSeatEventHandler, WlSeatRef},
        wl_shm::{WlShm, WlShmFormat},
        wl_surface::{WlSurface, WlSurfaceEventHandler, WlSurfaceRef},
    },
};

use clap::Parser;
use image::imageops::{self, FilterType};
use memfile::{MemFile, Seal};
use std::collections::HashMap;
use std::io::{self, Write};
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{self, Command, Stdio};
use std::thread;
use wl_client::proxy::OwnedProxy;
use wl_client::{Libwayland, proxy};

const BTN_LEFT: u32 = 0x110;
const BTN_RIGHT: u32 = 0x111;
const BTN_MIDDLE: u32 = 0x112;

#[derive(Parser)]
#[command(version, about = "Displays a custom item in Jay's system tray")]
struct Args {
    /// PNG or SVG file path, or a freedesktop icon name
    #[arg(long)]
    icon: Option<String>,

    /// Only show on this output, by connector name (see `jay randr`), e.g. DP-1
    #[arg(long, value_name = "NAME")]
    output: Option<String>,

    /// Shell command to run on left click
    #[arg(long, value_name = "CMD")]
    left_click: Option<String>,

    /// Shell command to run on right click
    #[arg(long, value_name = "CMD")]
    right_click: Option<String>,

    /// Shell command to run on middle click
    #[arg(long, value_name = "CMD")]
    middle_click: Option<String>,
}

/// One tray item per jay_tray_v1 global. Jay advertises one global per output.
struct Tray {
    tray_item: Option<JayTrayItemV1>,
    surface: Option<WlSurface>,
    current_buffer: Option<WlBuffer>,
    jay_tray: JayTrayV1,
    pending_size: Option<(i32, i32)>,
    size: (i32, i32),
    scale: i32,
    output_name: Option<String>,
}

impl Tray {
    fn create_item(&mut self, compositor: &WlCompositor, global: u32) {
        let surface = compositor.create_surface();
        proxy::set_event_handler(&surface, SurfaceHandler { tray: global });
        let tray_item = self.jay_tray.get_tray_item(&surface);
        proxy::set_event_handler(&tray_item, TrayItemHandler { tray: global });

        self.surface = Some(surface);
        self.tray_item = Some(tray_item);
    }
}

impl Drop for Tray {
    // wl-client's Drop frees the proxy without sending the destructor request.
    fn drop(&mut self) {
        // Jay rejects destroying a surface whose role object is still alive.
        if let Some(tray_item) = &self.tray_item {
            tray_item.destroy();
        }
        if let Some(surface) = &self.surface {
            surface.destroy();
        }
        if let Some(buffer) = &self.current_buffer {
            buffer.destroy();
        }
        self.jay_tray.destroy();
    }
}

struct Output {
    proxy: WlOutput,
    name: Option<String>,
}

struct Seat {
    proxy: WlSeat,
    pointer: Option<WlPointer>,
}

struct State {
    args: Args,
    registry: WlRegistry,
    compositor: Option<WlCompositor>,
    shm: Option<WlShm>,
    initialized: bool,
    trays: HashMap<u32, Tray>,
    outputs: HashMap<u32, Output>,
    seats: HashMap<u32, Seat>,
}

impl State {
    fn new(args: Args, registry: WlRegistry) -> Self {
        Self {
            args,
            registry,
            compositor: None,
            shm: None,
            initialized: false,
            trays: HashMap::new(),
            outputs: HashMap::new(),
            seats: HashMap::new(),
        }
    }

    fn initialize(&mut self) {
        let Some(compositor) = &self.compositor else {
            die("compositor does not advertise wl_compositor");
        };
        if self.shm.is_none() {
            die("compositor does not advertise wl_shm");
        }
        if self.trays.is_empty() {
            // Jay creates one global per connector, so a monitor plugged in later still adds one.
            eprintln!("No jay_tray_v1 yet, waiting for a monitor. Is this the Jay compositor?");
        }

        self.initialized = true;
        for (&global, tray) in &mut self.trays {
            tray.create_item(compositor, global);
        }
    }

    fn add_tray(&mut self, global: u32) {
        let mut tray = Tray {
            tray_item: None,
            surface: None,
            current_buffer: None,
            jay_tray: self.registry.bind(global, 1),
            pending_size: None,
            size: (0, 0),
            scale: 1,
            output_name: None,
        };
        if self.initialized
            && let Some(compositor) = &self.compositor
        {
            tray.create_item(compositor, global);
        }
        self.trays.insert(global, tray);
    }

    fn on_configure(&mut self, global: u32, serial: u32) {
        let Some(tray) = self.trays.get_mut(&global) else {
            return;
        };
        // Jay sends wl_surface.enter for the tray's output before the first configure.
        if let Some(wanted) = &self.args.output
            && tray.output_name.as_ref() != Some(wanted)
        {
            let found = tray.output_name.as_deref().unwrap_or("unnamed output");
            eprintln!("Skipping {found}: --output is {wanted}");
            self.trays.remove(&global);
            return;
        }
        if let Some(size) = tray.pending_size.take() {
            tray.size = size;
        }
        self.do_commit(global, Some(serial));
    }

    fn do_commit(&mut self, global: u32, serial: Option<u32>) {
        let Some(tray) = self.trays.get_mut(&global) else {
            return;
        };
        let (Some(shm), Some(surface), Some(tray_item)) =
            (&self.shm, &tray.surface, &tray.tray_item)
        else {
            return;
        };
        let Some((phys_w, phys_h)) = physical_size(tray.size, tray.scale) else {
            return;
        };

        let pixels = render_icon(self.args.icon.as_deref(), phys_w as u32, phys_h as u32);

        match create_shm_buffer(shm, &pixels, phys_w, phys_h) {
            Ok(buffer) => {
                if let Some(s) = serial {
                    tray_item.ack_configure(s);
                }
                surface.set_buffer_scale(tray.scale);
                surface.attach(Some(&*buffer), 0, 0);
                surface.damage_buffer(0, 0, i32::MAX, i32::MAX);
                surface.commit();
                if let Some(old) = tray.current_buffer.replace(buffer) {
                    old.destroy();
                }
            }
            Err(e) => eprintln!("Failed to create buffer: {e}"),
        }
    }

    fn handle_button(&self, button: u32) {
        let cmd = match button {
            BTN_LEFT => self.args.left_click.as_deref(),
            BTN_RIGHT => self.args.right_click.as_deref(),
            BTN_MIDDLE => self.args.middle_click.as_deref(),
            _ => None,
        };
        if let Some(cmd) = cmd {
            run_command(cmd);
        }
    }
}

fn die(msg: &str) -> ! {
    eprintln!("Error: {msg}");
    process::exit(1);
}

/// Buffer size for a tray, or None when the compositor sent values that can't make a
/// wl_shm buffer. Some values also guarantee that the pool size in bytes fits its i32.
fn physical_size((w, h): (i32, i32), scale: i32) -> Option<(i32, i32)> {
    if w <= 0 || h <= 0 || scale <= 0 {
        return None;
    }
    let phys_w = w.checked_mul(scale)?;
    let phys_h = h.checked_mul(scale)?;
    phys_w.checked_mul(phys_h)?.checked_mul(4)?;
    Some((phys_w, phys_h))
}

fn create_shm_buffer(shm: &WlShm, data: &[u8], width: i32, height: i32) -> io::Result<WlBuffer> {
    let mut memfd = MemFile::create_sealable("tray-icon")?;
    memfd.add_seal(Seal::Shrink)?;
    memfd.write_all(data)?;
    let pool = shm.create_pool(memfd.as_fd(), data.len() as i32);
    let buffer = pool.create_buffer(0, width, height, width * 4, WlShmFormat::ARGB8888);
    pool.destroy();
    Ok(buffer)
}

fn run_command(cmd: &str) {
    match Command::new("sh")
        .arg("-c")
        .arg(cmd)
        .stdin(Stdio::null())
        // Own process group: Ctrl+C in the tray's terminal must not kill launched apps.
        .process_group(0)
        .spawn()
    {
        // Reap in the background so finished commands don't stay as zombies.
        Ok(mut child) => {
            let reaper = thread::Builder::new().spawn(move || {
                let _ = child.wait();
            });
            if let Err(e) = reaper {
                eprintln!("Failed to start reaper thread: {e}");
            }
        }
        Err(e) => eprintln!("Failed to spawn command: {e}"),
    }
}

fn render_icon(icon_spec: Option<&str>, width: u32, height: u32) -> Vec<u8> {
    if let Some(spec) = icon_spec {
        if let Some(pixels) = load_icon(spec, width, height) {
            return pixels;
        }
        eprintln!("Could not load icon '{spec}', using fallback");
    }
    // Fallback: opaque dark gray square, as B, G, R, A
    [60, 60, 60, 255].repeat(width as usize * height as usize)
}

fn load_icon(spec: &str, width: u32, height: u32) -> Option<Vec<u8>> {
    let path = Path::new(spec);
    // is_file: opening a FIFO would block the event loop forever.
    if path.is_file()
        && let Some(pixels) = load_image_as_argb(path, width, height)
    {
        return Some(pixels);
    }
    // A path, not an icon name. `./firefox` must not resolve inside a theme directory.
    if spec.contains('/') {
        return None;
    }
    load_image_as_argb(&find_icon_file(spec, width)?, width, height)
}

fn load_image_as_argb(path: &Path, width: u32, height: u32) -> Option<Vec<u8>> {
    let is_svg = path
        .extension()
        .is_some_and(|e| e.eq_ignore_ascii_case("svg"));
    if is_svg {
        load_svg_as_argb(path, width, height)
    } else {
        load_png_as_argb(path, width, height)
    }
}

/// Converts premultiplied RGBA bytes to wl_shm ARGB8888, which is B, G, R, A on little-endian.
/// wl_buffer contents are premultiplied unless a protocol says otherwise.
fn premultiplied_rgba_to_argb(mut pixels: Vec<u8>) -> Vec<u8> {
    for px in pixels.as_chunks_mut::<4>().0 {
        px.swap(0, 2);
        let alpha = px[3];
        // Lanczos can overshoot, and a colour above its alpha is invalid when premultiplied.
        for channel in &mut px[..3] {
            *channel = (*channel).min(alpha);
        }
    }
    pixels
}

/// Largest size with the source's aspect ratio that fits the destination.
fn fit_within(src: (u32, u32), dst: (u32, u32)) -> (u32, u32) {
    let scale = (f64::from(dst.0) / f64::from(src.0)).min(f64::from(dst.1) / f64::from(src.1));
    let fit = |src: u32, dst: u32| ((f64::from(src) * scale).round() as u32).clamp(1, dst);
    (fit(src.0, dst.0), fit(src.1, dst.1))
}

fn load_png_as_argb(path: &Path, width: u32, height: u32) -> Option<Vec<u8>> {
    let mut img = image::ImageReader::open(path)
        .ok()?
        .with_guessed_format()
        .ok()?
        .decode()
        .ok()?
        .into_rgba8();
    // Premultiply before resizing so transparent pixels don't bleed colour into the edges.
    for px in img.pixels_mut() {
        let alpha = u16::from(px.0[3]);
        for channel in &mut px.0[..3] {
            *channel = ((u16::from(*channel) * alpha + 127) / 255) as u8;
        }
    }
    let (fit_w, fit_h) = fit_within(img.dimensions(), (width, height));
    let scaled = imageops::resize(&img, fit_w, fit_h, FilterType::Lanczos3);
    let mut canvas = image::RgbaImage::new(width, height);
    let (x, y) = ((width - fit_w) / 2, (height - fit_h) / 2);
    imageops::replace(&mut canvas, &scaled, i64::from(x), i64::from(y));
    Some(premultiplied_rgba_to_argb(canvas.into_raw()))
}

fn load_svg_as_argb(path: &Path, width: u32, height: u32) -> Option<Vec<u8>> {
    let data = std::fs::read(path).ok()?;
    let tree = resvg::usvg::Tree::from_data(&data, &resvg::usvg::Options::default()).ok()?;
    let mut pixmap = resvg::tiny_skia::Pixmap::new(width, height)?;
    let svg_size = tree.size();
    let scale = (width as f32 / svg_size.width()).min(height as f32 / svg_size.height());
    let x = (width as f32 - svg_size.width() * scale) / 2.0;
    let y = (height as f32 - svg_size.height() * scale) / 2.0;
    resvg::render(
        &tree,
        resvg::tiny_skia::Transform::from_row(scale, 0.0, 0.0, scale, x, y),
        &mut pixmap.as_mut(),
    );
    // resvg only logs what it can't draw, such as text, which this build leaves out.
    if pixmap.data().iter().all(|&b| b == 0) {
        return None;
    }
    Some(premultiplied_rgba_to_argb(pixmap.take()))
}

fn non_empty_env(name: &str) -> Option<String> {
    std::env::var(name).ok().filter(|v| !v.is_empty())
}

fn find_icon_file(name: &str, preferred_size: u32) -> Option<PathBuf> {
    let data_home = non_empty_env("XDG_DATA_HOME")
        .or_else(|| non_empty_env("HOME").map(|home| format!("{home}/.local/share")));
    let data_dirs =
        non_empty_env("XDG_DATA_DIRS").unwrap_or_else(|| "/usr/local/share:/usr/share".to_string());

    let bases: Vec<String> = data_home
        .into_iter()
        .chain(
            data_dirs
                .split(':')
                .filter(|d| !d.is_empty())
                .map(str::to_string),
        )
        .collect();

    find_icon_in_dirs(name, preferred_size, &bases)
}

/// Exact-size PNG, then SVG, then the nearest other PNG, then pixmaps. Each tier checks every
/// base, so a small icon in one base can't beat a better one in the next.
/// Larger PNGs come before smaller ones because shrinking loses less than enlarging.
fn find_icon_in_dirs(name: &str, preferred_size: u32, bases: &[String]) -> Option<PathBuf> {
    const SUBDIRS: [&str; 6] = [
        "apps",
        "status",
        "devices",
        "actions",
        "categories",
        "mimetypes",
    ];
    let mut sizes = [1024u32, 512, 256, 128, 96, 64, 48, 36, 32, 24, 22, 16];
    sizes.sort_by_key(|&s| (s < preferred_size, s.abs_diff(preferred_size)));

    let in_theme = |dir: &str, ext: &str| {
        bases.iter().find_map(|base| {
            SUBDIRS
                .iter()
                .map(|sub| PathBuf::from(format!("{base}/icons/hicolor/{dir}/{sub}/{name}.{ext}")))
                .find(|p| p.is_file())
        })
    };
    let png_at = |size: u32| in_theme(&format!("{size}x{size}"), "png");

    png_at(preferred_size)
        .or_else(|| in_theme("scalable", "svg"))
        .or_else(|| sizes.iter().find_map(|&size| png_at(size)))
        .or_else(|| {
            bases
                .iter()
                .map(|base| PathBuf::from(format!("{base}/pixmaps/{name}.png")))
                .find(|p| p.is_file())
        })
}

// --- Event handlers ---

struct RegistryHandler;

impl WlRegistryEventHandler for RegistryHandler {
    type Data = State;

    fn global(
        &self,
        state: &mut State,
        _slf: &WlRegistryRef,
        name: u32,
        interface: &str,
        version: u32,
    ) {
        match interface {
            WlCompositor::INTERFACE => {
                state.compositor = Some(state.registry.bind(name, version.min(6)));
            }
            WlShm::INTERFACE => {
                state.shm = Some(state.registry.bind(name, 1));
            }
            JayTrayV1::INTERFACE => state.add_tray(name),
            // wl_output.name needs version 4.
            WlOutput::INTERFACE if state.args.output.is_some() => {
                let proxy: WlOutput = state.registry.bind(name, version.min(4));
                proxy::set_event_handler(&proxy, OutputHandler { global: name });
                state.outputs.insert(name, Output { proxy, name: None });
            }
            WlSeat::INTERFACE => {
                let proxy: WlSeat = state.registry.bind(name, version.min(7));
                proxy::set_event_handler(&proxy, SeatHandler { global: name });
                state.seats.insert(
                    name,
                    Seat {
                        proxy,
                        pointer: None,
                    },
                );
            }
            _ => {}
        }
    }

    fn global_remove(&self, state: &mut State, _slf: &WlRegistryRef, name: u32) {
        state.trays.remove(&name);
        if let Some(output) = state.outputs.remove(&name) {
            output.proxy.release();
        }
        if let Some(seat) = state.seats.remove(&name) {
            if let Some(pointer) = &seat.pointer {
                pointer.release();
            }
            seat.proxy.release();
        }
    }
}

struct InitialRoundtrip;

impl WlCallbackEventHandler for InitialRoundtrip {
    type Data = State;

    fn done(&self, state: &mut State, _slf: &WlCallbackRef, _callback_data: u32) {
        state.initialize();
    }
}

struct OutputHandler {
    global: u32,
}

impl WlOutputEventHandler for OutputHandler {
    type Data = State;

    fn name(&self, state: &mut State, _slf: &WlOutputRef, name: &str) {
        if let Some(output) = state.outputs.get_mut(&self.global) {
            output.name = Some(name.to_string());
        }
    }
}

struct SurfaceHandler {
    tray: u32,
}

impl WlSurfaceEventHandler for SurfaceHandler {
    type Data = State;

    fn enter(&self, state: &mut State, _slf: &WlSurfaceRef, output: Option<&WlOutputRef>) {
        let Some(output) = output else { return };
        let name = state
            .outputs
            .values()
            .find(|o| proxy::id(&*o.proxy) == proxy::id(output))
            .and_then(|o| o.name.clone());
        if let Some(tray) = state.trays.get_mut(&self.tray) {
            tray.output_name = name;
        }
    }

    fn preferred_buffer_scale(&self, state: &mut State, _slf: &WlSurfaceRef, factor: i32) {
        let Some(tray) = state.trays.get_mut(&self.tray) else {
            return;
        };
        if tray.scale != factor {
            tray.scale = factor;
            state.do_commit(self.tray, None);
        }
    }
}

struct TrayItemHandler {
    tray: u32,
}

impl JayTrayItemV1EventHandler for TrayItemHandler {
    type Data = State;

    fn configure_size(&self, state: &mut State, _slf: &JayTrayItemV1Ref, width: i32, height: i32) {
        if let Some(tray) = state.trays.get_mut(&self.tray) {
            tray.pending_size = Some((width, height));
        }
    }

    fn configure(&self, state: &mut State, _slf: &JayTrayItemV1Ref, serial: u32) {
        state.on_configure(self.tray, serial);
    }
}

struct SeatHandler {
    global: u32,
}

impl WlSeatEventHandler for SeatHandler {
    type Data = State;

    fn capabilities(&self, state: &mut State, _slf: &WlSeatRef, capabilities: WlSeatCapability) {
        let Some(seat) = state.seats.get_mut(&self.global) else {
            return;
        };
        if !capabilities.contains(WlSeatCapability::POINTER) {
            if let Some(pointer) = seat.pointer.take() {
                pointer.release();
            }
        } else if seat.pointer.is_none() {
            let pointer = seat.proxy.get_pointer();
            proxy::set_event_handler(&pointer, PointerHandler);
            seat.pointer = Some(pointer);
        }
    }
}

struct PointerHandler;

impl WlPointerEventHandler for PointerHandler {
    type Data = State;

    // No hover tracking needed: buttons only reach the surface under the pointer, and every
    // surface this client owns is a tray item.
    fn button(
        &self,
        state: &mut State,
        _slf: &WlPointerRef,
        _serial: u32,
        _time: u32,
        button: u32,
        button_state: WlPointerButtonState,
    ) {
        if button_state == WlPointerButtonState::PRESSED {
            state.handle_button(button);
        }
    }
}

fn main() {
    let args = Args::parse();

    let lib = Libwayland::open()
        .unwrap_or_else(|e| die(&format!("failed to load libwayland-client: {e}")));
    let conn = lib
        .connect_to_default_display()
        .unwrap_or_else(|e| die(&format!("failed to connect to Wayland display: {e}")));

    let (_owner, queue) = conn.create_queue_with_data::<State>(c"jay-tray-item");
    let display = queue.display::<WlDisplay>();
    let registry = display.get_registry();
    proxy::set_event_handler(&registry, RegistryHandler);

    let sync_cb = display.sync();
    proxy::set_event_handler(&sync_cb, InitialRoundtrip);

    let mut state = State::new(args, registry);

    loop {
        if let Err(e) = queue.dispatch_blocking(&mut state) {
            die(&format!("Wayland dispatch failed: {e}"));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn write_test_png(path: &Path, width: u32, height: u32, rgba: [u8; 4]) {
        let mut img = image::RgbaImage::new(width, height);
        for pixel in img.pixels_mut() {
            *pixel = image::Rgba(rgba);
        }
        img.save(path).unwrap();
    }

    fn minimal_svg(fill: &str) -> String {
        format!(
            r#"<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100"><rect width="100" height="100" fill="{fill}"/></svg>"#
        )
    }

    fn tmp(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!("jay-tray-test-{}-{name}", process::id()))
    }

    fn alpha_at(pixels: &[u8], width: usize, x: usize, y: usize) -> u8 {
        pixels[(y * width + x) * 4 + 3]
    }

    // --- PNG ---

    #[test]
    fn png_exact_size_returns_correct_byte_count() {
        let p = tmp("png_exact.png");
        write_test_png(&p, 32, 32, [0, 255, 0, 255]);
        let out = load_png_as_argb(&p, 32, 32).unwrap();
        assert_eq!(out.len(), 32 * 32 * 4);
        fs::remove_file(p).ok();
    }

    #[test]
    fn png_scales_to_target_dimensions() {
        let p = tmp("png_scale.png");
        write_test_png(&p, 64, 64, [0, 0, 255, 255]);
        let out = load_png_as_argb(&p, 24, 24).unwrap();
        assert_eq!(out.len(), 24 * 24 * 4);
        fs::remove_file(p).ok();
    }

    #[test]
    fn png_pixel_bytes_are_argb8888_little_endian() {
        // Pure red, opaque → ARGB8888 LE bytes: B=0 G=0 R=255 A=255
        let p = tmp("png_pixel.png");
        write_test_png(&p, 1, 1, [255, 0, 0, 255]);
        let out = load_png_as_argb(&p, 1, 1).unwrap();
        assert_eq!(out, &[0, 0, 255, 255]);
        fs::remove_file(p).ok();
    }

    #[test]
    fn png_semi_transparent_pixels_are_premultiplied() {
        let p = tmp("png_alpha.png");
        write_test_png(&p, 1, 1, [255, 0, 0, 128]);
        let out = load_png_as_argb(&p, 1, 1).unwrap();
        assert_eq!(out, &[0, 0, 128, 128]);
        fs::remove_file(p).ok();
    }

    #[test]
    fn png_resize_keeps_colour_at_or_below_alpha() {
        // Lanczos overshoots at a hard edge between a dim colour and an opaque black.
        let p = tmp("png_overshoot.png");
        let mut img = image::RgbaImage::new(64, 64);
        for (x, _, pixel) in img.enumerate_pixels_mut() {
            *pixel = image::Rgba(if x < 32 {
                [255, 0, 0, 128]
            } else {
                [0, 0, 0, 255]
            });
        }
        img.save(&p).unwrap();
        let out = load_png_as_argb(&p, 24, 24).unwrap();
        for px in out.as_chunks::<4>().0 {
            assert!(px[..3].iter().all(|&c| c <= px[3]), "invalid pixel {px:?}");
        }
        fs::remove_file(p).ok();
    }

    #[test]
    fn png_non_square_is_centered_without_stretching() {
        let p = tmp("png_wide.png");
        write_test_png(&p, 4, 2, [255, 0, 0, 255]);
        let out = load_png_as_argb(&p, 4, 4).unwrap();
        assert_eq!(out.len(), 4 * 4 * 4);
        assert_eq!(alpha_at(&out, 4, 0, 0), 0);
        assert_eq!(alpha_at(&out, 4, 0, 1), 255);
        assert_eq!(alpha_at(&out, 4, 0, 2), 255);
        assert_eq!(alpha_at(&out, 4, 0, 3), 0);
        fs::remove_file(p).ok();
    }

    #[test]
    fn png_missing_file_returns_none() {
        let out = load_png_as_argb(Path::new("/no/such/icon.png"), 32, 32);
        assert!(out.is_none());
    }

    // --- SVG ---

    #[test]
    fn svg_renders_to_correct_byte_count() {
        let p = tmp("svg_size.svg");
        fs::write(&p, minimal_svg("red")).unwrap();
        let out = load_svg_as_argb(&p, 24, 24).unwrap();
        assert_eq!(out.len(), 24 * 24 * 4);
        fs::remove_file(p).ok();
    }

    #[test]
    fn svg_opaque_fill_produces_full_alpha() {
        let p = tmp("svg_alpha.svg");
        fs::write(&p, minimal_svg("blue")).unwrap();
        let out = load_svg_as_argb(&p, 4, 4).unwrap();
        // Every pixel of a viewport-filling opaque rect must have alpha=255
        for chunk in out.chunks(4) {
            assert_eq!(chunk[3], 255, "expected alpha=255, got {:?}", chunk);
        }
        fs::remove_file(p).ok();
    }

    #[test]
    fn svg_that_draws_nothing_counts_as_failed() {
        let p = tmp("svg_blank.svg");
        fs::write(
            &p,
            r#"<svg xmlns="http://www.w3.org/2000/svg" width="10" height="10"/>"#,
        )
        .unwrap();
        assert!(load_svg_as_argb(&p, 4, 4).is_none());
        fs::remove_file(p).ok();
    }

    #[test]
    fn svg_draws_embedded_png() {
        const PNG: &str = "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mP8z8DwHwAFBQIAX8jx0gAAAABJRU5ErkJggg==";
        let p = tmp("svg_embedded.svg");
        fs::write(
            &p,
            format!(
                r#"<svg xmlns="http://www.w3.org/2000/svg" xmlns:xlink="http://www.w3.org/1999/xlink" width="4" height="4"><image width="4" height="4" xlink:href="data:image/png;base64,{PNG}"/></svg>"#
            ),
        )
        .unwrap();
        let out = load_svg_as_argb(&p, 4, 4).unwrap();
        assert!(out.as_chunks::<4>().0.iter().any(|px| px[3] > 0));
        fs::remove_file(p).ok();
    }

    #[test]
    fn svg_semi_transparent_pixels_stay_premultiplied() {
        let p = tmp("svg_premul.svg");
        fs::write(&p, minimal_svg(r#"red" fill-opacity="0.5"#)).unwrap();
        let out = load_svg_as_argb(&p, 1, 1).unwrap();
        assert_eq!(out[0], 0, "blue");
        assert_eq!(out[1], 0, "green");
        assert!(
            out[2].abs_diff(out[3]) <= 1,
            "red {} vs alpha {}",
            out[2],
            out[3]
        );
        assert!(out[3].abs_diff(128) <= 1, "alpha {}", out[3]);
        fs::remove_file(p).ok();
    }

    #[test]
    fn svg_non_square_is_centered_without_stretching() {
        let p = tmp("svg_wide.svg");
        fs::write(
            &p,
            r#"<svg xmlns="http://www.w3.org/2000/svg" width="4" height="2"><rect width="4" height="2" fill="red"/></svg>"#,
        )
        .unwrap();
        let out = load_svg_as_argb(&p, 4, 4).unwrap();
        assert_eq!(alpha_at(&out, 4, 0, 0), 0);
        assert_eq!(alpha_at(&out, 4, 0, 1), 255);
        assert_eq!(alpha_at(&out, 4, 0, 2), 255);
        assert_eq!(alpha_at(&out, 4, 0, 3), 0);
        fs::remove_file(p).ok();
    }

    #[test]
    fn svg_missing_file_returns_none() {
        let out = load_svg_as_argb(Path::new("/no/such/icon.svg"), 32, 32);
        assert!(out.is_none());
    }

    // --- load_image_as_argb dispatch ---

    #[test]
    fn dispatch_routes_svg_extension_to_svg_loader() {
        let p = tmp("dispatch.svg");
        fs::write(&p, minimal_svg("green")).unwrap();
        assert!(load_image_as_argb(&p, 8, 8).is_some());
        fs::remove_file(p).ok();
    }

    #[test]
    fn dispatch_treats_uppercase_svg_extension_as_svg() {
        let p = tmp("dispatch_upper.SVG");
        fs::write(&p, minimal_svg("green")).unwrap();
        assert!(load_image_as_argb(&p, 8, 8).is_some());
        fs::remove_file(p).ok();
    }

    #[test]
    fn dispatch_routes_png_extension_to_png_loader() {
        let p = tmp("dispatch.png");
        write_test_png(&p, 8, 8, [128, 128, 128, 255]);
        assert!(load_image_as_argb(&p, 8, 8).is_some());
        fs::remove_file(p).ok();
    }

    #[test]
    fn png_is_recognised_without_its_extension() {
        let named = tmp("no_ext.png");
        let bare = tmp("no_ext");
        write_test_png(&named, 2, 2, [1, 2, 3, 255]);
        fs::rename(&named, &bare).unwrap();
        assert!(load_image_as_argb(&bare, 2, 2).is_some());
        fs::remove_file(bare).ok();
    }

    #[test]
    fn non_image_file_returns_none() {
        let p = tmp("not_an_image");
        fs::write(&p, "#!/bin/sh\n").unwrap();
        assert!(load_image_as_argb(&p, 2, 2).is_none());
        fs::remove_file(p).ok();
    }

    // --- load_icon ---

    #[test]
    fn load_icon_loads_a_path() {
        let p = tmp("direct.png");
        write_test_png(&p, 2, 2, [1, 2, 3, 255]);
        assert!(load_icon(p.to_str().unwrap(), 2, 2).is_some());
        fs::remove_file(p).ok();
    }

    #[test]
    fn load_icon_rejects_a_path_that_is_not_a_file() {
        let dir = tmp("not_a_file");
        fs::create_dir_all(&dir).unwrap();
        assert!(load_icon(dir.to_str().unwrap(), 2, 2).is_none());
        fs::remove_dir_all(dir).ok();
    }

    // --- render_icon ---

    #[test]
    fn fallback_is_opaque_dark_gray() {
        assert_eq!(render_icon(None, 1, 2), [60, 60, 60, 255].repeat(2));
    }

    // --- sizes ---

    #[test]
    fn physical_size_multiplies_by_scale() {
        assert_eq!(physical_size((24, 32), 2), Some((48, 64)));
    }

    #[test]
    fn physical_size_rejects_values_that_cannot_make_a_buffer() {
        assert_eq!(physical_size((0, 24), 1), None);
        assert_eq!(physical_size((24, -1), 1), None);
        assert_eq!(physical_size((24, 24), 0), None);
        assert_eq!(physical_size((i32::MAX, 1), 2), None);
        // 32768 x 32768 x 4 bytes overflows the i32 pool size.
        assert_eq!(physical_size((32768, 32768), 1), None);
    }

    #[test]
    fn fit_within_keeps_aspect_ratio() {
        assert_eq!(fit_within((100, 50), (24, 24)), (24, 12));
        assert_eq!(fit_within((50, 100), (24, 24)), (12, 24));
        assert_eq!(fit_within((10, 10), (24, 24)), (24, 24));
    }

    #[test]
    fn fit_within_never_collapses_to_zero() {
        assert_eq!(fit_within((1000, 1), (24, 24)), (24, 1));
    }

    // --- find_icon_in_dirs ---

    fn touch(base: &Path, relative: &str) -> PathBuf {
        let path = base.join(relative);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, "").unwrap();
        path
    }

    fn find_in(base: &Path, name: &str, size: u32) -> Option<PathBuf> {
        find_icon_in_dirs(name, size, &[base.to_str().unwrap().to_string()])
    }

    #[test]
    fn find_icon_locates_sized_png() {
        let base = tmp("theme_png");
        let icon = touch(&base, "icons/hicolor/32x32/apps/x.png");
        let found = find_in(&base, "x", 32);
        fs::remove_dir_all(&base).ok();
        assert_eq!(found, Some(icon));
    }

    #[test]
    fn find_icon_locates_scalable_svg() {
        let base = tmp("theme_svg");
        let icon = touch(&base, "icons/hicolor/scalable/apps/x.svg");
        let found = find_in(&base, "x", 32);
        fs::remove_dir_all(&base).ok();
        assert_eq!(found, Some(icon));
    }

    #[test]
    fn find_icon_prefers_exact_png_over_scalable_svg() {
        let base = tmp("theme_exact");
        let png = touch(&base, "icons/hicolor/32x32/apps/x.png");
        touch(&base, "icons/hicolor/scalable/apps/x.svg");
        let found = find_in(&base, "x", 32);
        fs::remove_dir_all(&base).ok();
        assert_eq!(found, Some(png));
    }

    #[test]
    fn find_icon_prefers_scalable_svg_over_other_png_sizes() {
        let base = tmp("theme_blur");
        touch(&base, "icons/hicolor/16x16/apps/x.png");
        let svg = touch(&base, "icons/hicolor/scalable/apps/x.svg");
        let found = find_in(&base, "x", 48);
        fs::remove_dir_all(&base).ok();
        assert_eq!(found, Some(svg));
    }

    #[test]
    fn find_icon_prefers_larger_png_over_smaller() {
        let base = tmp("theme_larger");
        touch(&base, "icons/hicolor/24x24/apps/x.png");
        let larger = touch(&base, "icons/hicolor/48x48/apps/x.png");
        let found = find_in(&base, "x", 32);
        fs::remove_dir_all(&base).ok();
        assert_eq!(found, Some(larger));
    }

    #[test]
    fn find_icon_uses_exact_size_outside_the_common_list() {
        let base = tmp("theme_uncommon");
        touch(&base, "icons/hicolor/48x48/apps/x.png");
        let exact = touch(&base, "icons/hicolor/30x30/apps/x.png");
        let found = find_in(&base, "x", 30);
        fs::remove_dir_all(&base).ok();
        assert_eq!(found, Some(exact));
    }

    #[test]
    fn find_icon_falls_back_to_pixmaps() {
        let base = tmp("theme_pixmap");
        let icon = touch(&base, "pixmaps/x.png");
        let found = find_in(&base, "x", 32);
        fs::remove_dir_all(&base).ok();
        assert_eq!(found, Some(icon));
    }

    #[test]
    fn find_icon_ignores_directories_named_like_icons() {
        let base = tmp("theme_dir");
        fs::create_dir_all(base.join("pixmaps/x.png")).unwrap();
        let found = find_in(&base, "x", 32);
        fs::remove_dir_all(&base).ok();
        assert_eq!(found, None);
    }

    fn find_across(bases: &[&Path], name: &str, size: u32) -> Option<PathBuf> {
        let bases: Vec<String> = bases
            .iter()
            .map(|b| b.to_str().unwrap().to_string())
            .collect();
        find_icon_in_dirs(name, size, &bases)
    }

    #[test]
    fn find_icon_tiers_span_bases() {
        let a = tmp("theme_tier_a");
        let b = tmp("theme_tier_b");
        touch(&a, "icons/hicolor/16x16/apps/x.png");
        touch(&a, "pixmaps/x.png");
        let exact = touch(&b, "icons/hicolor/48x48/apps/x.png");
        let found = find_across(&[&a, &b], "x", 48);
        fs::remove_dir_all(&a).ok();
        fs::remove_dir_all(&b).ok();
        assert_eq!(found, Some(exact));
    }

    #[test]
    fn find_icon_earlier_base_wins_within_a_tier() {
        let a = tmp("theme_order_a");
        let b = tmp("theme_order_b");
        let first = touch(&a, "icons/hicolor/48x48/apps/x.png");
        touch(&b, "icons/hicolor/48x48/apps/x.png");
        let found = find_across(&[&a, &b], "x", 48);
        fs::remove_dir_all(&a).ok();
        fs::remove_dir_all(&b).ok();
        assert_eq!(found, Some(first));
    }

    #[test]
    fn find_icon_finds_very_large_pngs() {
        let base = tmp("theme_large");
        let icon = touch(&base, "icons/hicolor/512x512/apps/x.png");
        let found = find_in(&base, "x", 24);
        fs::remove_dir_all(&base).ok();
        assert_eq!(found, Some(icon));
    }

    #[test]
    fn find_icon_returns_none_for_unknown_name() {
        let bases = vec!["/tmp".to_string()];
        assert!(find_icon_in_dirs("__no_such_icon_xyzzy__", 32, &bases).is_none());
    }
}
