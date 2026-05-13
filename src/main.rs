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
        wl_pointer::{WlPointer, WlPointerButtonState, WlPointerEventHandler, WlPointerRef},
        wl_registry::{WlRegistry, WlRegistryEventHandler, WlRegistryRef},
        wl_seat::{WlSeat, WlSeatCapability, WlSeatEventHandler, WlSeatRef},
        wl_shm::{WlShm, WlShmFormat},
        wl_surface::{WlSurface, WlSurfaceEventHandler, WlSurfaceRef},
    },
    xdg_shell::xdg_positioner::{XdgPositionerAnchor, XdgPositionerGravity},
};

use clap::Parser;
use image::imageops::FilterType;
use memfile::{MemFile, Seal};
use std::io::{self, Write};
use std::ops::Deref;
use std::path::{Path, PathBuf};
use wl_client::{Fixed, Libwayland, proxy};
use wl_client::proxy::OwnedProxy;

const BTN_LEFT: u32 = 0x110;
const BTN_RIGHT: u32 = 0x111;
const BTN_MIDDLE: u32 = 0x112;

#[derive(Parser)]
#[command(version, about = "Displays a custom item in Jay's system tray")]
struct Args {
    /// PNG file path or freedesktop icon name
    #[arg(long)]
    icon: Option<String>,

    /// Tooltip text (reserved for future implementation)
    #[arg(long)]
    tooltip: Option<String>,

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

struct State {
    registry: WlRegistry,
    compositor: Option<WlCompositor>,
    shm: Option<WlShm>,
    jay_tray: Option<JayTrayV1>,
    seat: Option<WlSeat>,
    pointer: Option<WlPointer>,
    surface: Option<WlSurface>,
    tray_item: Option<JayTrayItemV1>,
    pending_size: Option<(i32, i32)>,
    size: (i32, i32),
    scale: i32,
    pointer_on_surface: bool,
    current_buffer: Option<WlBuffer>,
    icon: Option<String>,
    left_click: Option<String>,
    right_click: Option<String>,
    middle_click: Option<String>,
}

impl State {
    fn new(args: Args, registry: WlRegistry) -> Self {
        if args.tooltip.is_some() {
            log::warn!("--tooltip is accepted but not yet displayed");
        }
        Self {
            registry,
            compositor: None,
            shm: None,
            jay_tray: None,
            seat: None,
            pointer: None,
            surface: None,
            tray_item: None,
            pending_size: None,
            size: (0, 0),
            scale: 1,
            pointer_on_surface: false,
            current_buffer: None,
            icon: args.icon,
            left_click: args.left_click,
            right_click: args.right_click,
            middle_click: args.middle_click,
        }
    }

    fn initialize(&mut self) {
        macro_rules! require {
            ($field:ident, $name:literal) => {
                match &self.$field {
                    Some(v) => v.clone(),
                    None => {
                        eprintln!("Error: compositor does not advertise {}", $name);
                        std::process::exit(1);
                    }
                }
            };
        }

        let compositor = require!(compositor, "wl_compositor");
        let jay_tray = require!(jay_tray, "jay_tray_v1 — is this the Jay compositor?");

        let surface = compositor.create_surface();
        proxy::set_event_handler(&surface, SurfaceHandler);
        let tray_item = jay_tray.get_tray_item(&surface);
        proxy::set_event_handler(&tray_item, TrayItemHandler);

        self.surface = Some(surface);
        self.tray_item = Some(tray_item);
    }

    fn setup_pointer(&mut self) {
        let Some(seat) = self.seat.clone() else {
            return;
        };
        let pointer = seat.get_pointer();
        proxy::set_event_handler(&pointer, PointerHandler);
        self.pointer = Some(pointer);
    }

    fn on_configure(&mut self, serial: u32) {
        if let Some(size) = self.pending_size.take() {
            self.size = size;
        }
        self.do_commit(Some(serial));
    }

    fn do_commit(&mut self, serial: Option<u32>) {
        let (w, h) = self.size;
        if w == 0 || h == 0 {
            return;
        }

        let Some(shm) = self.shm.clone() else { return };
        let Some(surface) = self.surface.clone() else { return };
        let Some(tray_item) = self.tray_item.clone() else { return };

        let phys_w = w * self.scale;
        let phys_h = h * self.scale;
        let pixels = render_icon(self.icon.as_deref(), phys_w as u32, phys_h as u32);

        match create_shm_buffer(&shm, &pixels, phys_w, phys_h) {
            Ok(buffer) => {
                if let Some(s) = serial {
                    tray_item.ack_configure(s);
                }
                surface.set_buffer_scale(self.scale);
                surface.attach(Some(buffer.deref()), 0, 0);
                surface.damage_buffer(0, 0, i32::MAX, i32::MAX);
                surface.commit();
                self.current_buffer = Some(buffer);
            }
            Err(e) => eprintln!("Failed to create buffer: {e}"),
        }
    }

    fn handle_button(&self, button: u32) {
        let cmd = match button {
            BTN_LEFT => self.left_click.as_deref(),
            BTN_RIGHT => self.right_click.as_deref(),
            BTN_MIDDLE => self.middle_click.as_deref(),
            _ => None,
        };
        if let Some(cmd) = cmd {
            run_command(cmd);
        }
    }
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
    if let Err(e) = std::process::Command::new("sh")
        .arg("-c")
        .arg(cmd)
        .spawn()
    {
        eprintln!("Failed to spawn command: {e}");
    }
}

fn render_icon(icon_spec: Option<&str>, width: u32, height: u32) -> Vec<u8> {
    if width == 0 || height == 0 {
        return Vec::new();
    }
    if let Some(spec) = icon_spec {
        if let Some(pixels) = load_icon(spec, width, height) {
            return pixels;
        }
        log::warn!("Could not load icon '{spec}', using fallback");
    }
    // Fallback: opaque dark gray square
    let mut buf = vec![0u8; (width * height * 4) as usize];
    for chunk in buf.chunks_mut(4) {
        chunk[0] = 60; // B
        chunk[1] = 60; // G
        chunk[2] = 60; // R
        chunk[3] = 255; // A
    }
    buf
}

fn load_icon(spec: &str, width: u32, height: u32) -> Option<Vec<u8>> {
    let path = Path::new(spec);
    if path.exists() {
        return load_png_as_argb(path, width, height);
    }
    let found = find_icon_file(spec, width as i32)?;
    load_png_as_argb(&found, width, height)
}

fn load_png_as_argb(path: &Path, width: u32, height: u32) -> Option<Vec<u8>> {
    let img = image::open(path).ok()?.into_rgba8();
    let img = if img.width() == width && img.height() == height {
        img
    } else {
        image::imageops::resize(&img, width, height, FilterType::Lanczos3)
    };
    let mut argb = Vec::with_capacity((width * height * 4) as usize);
    for pixel in img.pixels() {
        let [r, g, b, a] = pixel.0;
        // ARGB8888 on little-endian: bytes are B, G, R, A
        argb.extend_from_slice(&[b, g, r, a]);
    }
    Some(argb)
}

fn find_icon_file(name: &str, preferred_size: i32) -> Option<PathBuf> {
    let home = std::env::var("HOME").ok()?;
    let xdg_data_home = std::env::var("XDG_DATA_HOME")
        .unwrap_or_else(|_| format!("{home}/.local/share"));
    let xdg_data_dirs = std::env::var("XDG_DATA_DIRS")
        .unwrap_or_else(|_| "/usr/local/share:/usr/share".to_string());

    let mut bases: Vec<String> = vec![xdg_data_home];
    bases.extend(xdg_data_dirs.split(':').map(str::to_string));

    let mut sizes: Vec<i32> = vec![256, 128, 96, 64, 48, 36, 32, 24, 22, 16];
    if !sizes.contains(&preferred_size) {
        sizes.push(preferred_size);
    }
    sizes.sort_by_key(|&s| (s - preferred_size).abs());

    let subdirs = ["apps", "status", "devices", "actions", "categories", "mimetypes"];

    for base in &bases {
        for &size in &sizes {
            for subdir in &subdirs {
                let p = PathBuf::from(format!(
                    "{base}/icons/hicolor/{size}x{size}/{subdir}/{name}.png"
                ));
                if p.exists() {
                    return Some(p);
                }
            }
        }
        let p = PathBuf::from(format!("{base}/pixmaps/{name}.png"));
        if p.exists() {
            return Some(p);
        }
    }
    None
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
            JayTrayV1::INTERFACE => {
                state.jay_tray = Some(state.registry.bind(name, 1));
            }
            WlSeat::INTERFACE if state.seat.is_none() => {
                let seat: WlSeat = state.registry.bind(name, version.min(7));
                proxy::set_event_handler(&seat, SeatHandler);
                state.seat = Some(seat);
            }
            _ => {}
        }
    }

    fn global_remove(&self, _state: &mut State, _slf: &WlRegistryRef, _name: u32) {}
}

struct InitialRoundtrip;

impl WlCallbackEventHandler for InitialRoundtrip {
    type Data = State;

    fn done(&self, state: &mut State, _slf: &WlCallbackRef, _callback_data: u32) {
        state.initialize();
        if state.seat.is_some() && state.pointer.is_none() {
            state.setup_pointer();
        }
    }
}

struct SurfaceHandler;

impl WlSurfaceEventHandler for SurfaceHandler {
    type Data = State;

    fn preferred_buffer_scale(&self, state: &mut State, _slf: &WlSurfaceRef, factor: i32) {
        if state.scale != factor {
            state.scale = factor;
            state.do_commit(None);
        }
    }
}

struct TrayItemHandler;

impl JayTrayItemV1EventHandler for TrayItemHandler {
    type Data = State;

    fn configure_size(
        &self,
        state: &mut State,
        _slf: &JayTrayItemV1Ref,
        width: i32,
        height: i32,
    ) {
        state.pending_size = Some((width, height));
    }

    fn preferred_anchor(
        &self,
        _state: &mut State,
        _slf: &JayTrayItemV1Ref,
        _anchor: XdgPositionerAnchor,
    ) {
    }

    fn preferred_gravity(
        &self,
        _state: &mut State,
        _slf: &JayTrayItemV1Ref,
        _gravity: XdgPositionerGravity,
    ) {
    }

    fn configure(&self, state: &mut State, _slf: &JayTrayItemV1Ref, serial: u32) {
        state.on_configure(serial);
    }
}

struct SeatHandler;

impl WlSeatEventHandler for SeatHandler {
    type Data = State;

    fn capabilities(
        &self,
        state: &mut State,
        _slf: &WlSeatRef,
        capabilities: WlSeatCapability,
    ) {
        if capabilities.contains(WlSeatCapability::POINTER) && state.pointer.is_none() {
            state.setup_pointer();
        }
    }
}

struct PointerHandler;

impl WlPointerEventHandler for PointerHandler {
    type Data = State;

    fn enter(
        &self,
        state: &mut State,
        _slf: &WlPointerRef,
        _serial: u32,
        _surface: Option<&WlSurfaceRef>,
        _surface_x: Fixed,
        _surface_y: Fixed,
    ) {
        state.pointer_on_surface = true;
    }

    fn leave(
        &self,
        state: &mut State,
        _slf: &WlPointerRef,
        _serial: u32,
        _surface: Option<&WlSurfaceRef>,
    ) {
        state.pointer_on_surface = false;
    }

    fn button(
        &self,
        state: &mut State,
        _slf: &WlPointerRef,
        _serial: u32,
        _time: u32,
        button: u32,
        button_state: WlPointerButtonState,
    ) {
        if button_state != WlPointerButtonState::PRESSED {
            return;
        }
        if state.pointer_on_surface {
            state.handle_button(button);
        }
    }
}

fn main() {
    env_logger::init();
    let args = Args::parse();

    let lib = match Libwayland::open() {
        Ok(l) => l,
        Err(e) => {
            eprintln!("Failed to load libwayland-client: {e}");
            std::process::exit(1);
        }
    };
    let conn = match lib.connect_to_default_display() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("Failed to connect to Wayland display: {e}");
            std::process::exit(1);
        }
    };

    let (_owner, queue) = conn.create_queue_with_data::<State>(c"custom-jay-tray-item");
    let display = queue.display::<WlDisplay>();
    let registry = display.get_registry();
    proxy::set_event_handler(&registry, RegistryHandler);

    let sync_cb = display.sync();
    proxy::set_event_handler(&sync_cb, InitialRoundtrip);

    let mut state = State::new(args, registry);

    loop {
        if let Err(e) = queue.dispatch_blocking(&mut state) {
            eprintln!("Wayland dispatch error: {e}");
            std::process::exit(1);
        }
    }
}
