use std::{
    fs::File,
    io::{Seek, SeekFrom, Write},
    os::fd::{AsFd, FromRawFd},
    process,
};

use cairo::{Context, FontSlant, FontWeight, Format, ImageSurface, Operator};
use wayland_client::{
    delegate_noop,
    globals::{registry_queue_init, BindError, GlobalListContents},
    protocol::{
        wl_buffer::WlBuffer, wl_callback::WlCallback, wl_compositor::WlCompositor,
        wl_output::WlOutput, wl_region::WlRegion, wl_registry::WlRegistry, wl_shm::WlShm,
        wl_shm_pool::WlShmPool, wl_surface::WlSurface,
    },
    Connection, Dispatch, Proxy, QueueHandle,
};
use wayland_protocols_wlr::layer_shell::v1::client::{
    zwlr_layer_shell_v1::{Layer, ZwlrLayerShellV1},
    zwlr_layer_surface_v1::{
        Anchor, Event as LayerSurfaceEvent, KeyboardInteractivity, ZwlrLayerSurfaceV1,
    },
};

const TEXT_ALPHA: f64 = 0x50 as f64 / 255.0;
const RIGHT_MARGIN: i32 = 50;
const BOTTOM_MARGIN: i32 = 50;
const LINE_GAP: f64 = 5.0;

struct App {
    compositor: WlCompositor,
    shm: WlShm,
    overlays: Vec<Overlay>,
}

struct Overlay {
    surface: WlSurface,
    _layer_surface: ZwlrLayerSurfaceV1,
    buffer: Option<WlBuffer>,
    _shm_file: Option<File>,
}

struct LayerSurfaceData {
    index: usize,
}

struct RenderedWatermark {
    width: i32,
    height: i32,
    stride: i32,
    pixels: Vec<u8>,
}

fn main() {
    if let Err(err) = run() {
        eprintln!("activate-linux: {err}");
        process::exit(1);
    }
}

fn run() -> Result<(), Box<dyn std::error::Error>> {
    let conn = Connection::connect_to_env()?;
    let (globals, mut event_queue) = registry_queue_init::<App>(&conn)?;
    let qh = event_queue.handle();

    let compositor = bind::<WlCompositor>(&globals, &qh, 4..=6)?;
    let shm = bind::<WlShm>(&globals, &qh, 1..=1)?;
    let layer_shell = bind::<ZwlrLayerShellV1>(&globals, &qh, 1..=4)?;
    let outputs = globals
        .contents()
        .clone_list()
        .into_iter()
        .filter(|global| global.interface == WlOutput::interface().name)
        .map(|global| {
            let version = global.version.min(WlOutput::interface().version);
            globals.bind::<WlOutput, _, _>(&qh, version..=version, ())
        })
        .collect::<Result<Vec<_>, BindError>>()?;

    if outputs.is_empty() {
        return Err("compositor did not advertise any wl_output globals".into());
    }

    let mut app = App {
        compositor,
        shm,
        overlays: Vec::with_capacity(outputs.len()),
    };

    for output in outputs {
        create_overlay(&mut app, &layer_shell, &output, &qh);
    }

    loop {
        event_queue.blocking_dispatch(&mut app)?;
    }
}

fn bind<I>(
    globals: &wayland_client::globals::GlobalList,
    qh: &QueueHandle<App>,
    version: std::ops::RangeInclusive<u32>,
) -> Result<I, BindError>
where
    I: wayland_client::Proxy + 'static,
    App: Dispatch<I, ()>,
{
    globals.bind::<I, _, _>(qh, version, ())
}

fn create_overlay(
    app: &mut App,
    layer_shell: &ZwlrLayerShellV1,
    output: &WlOutput,
    qh: &QueueHandle<App>,
) {
    let index = app.overlays.len();
    let surface = app.compositor.create_surface(qh, ());
    let layer_surface = layer_shell.get_layer_surface(
        &surface,
        Some(output),
        Layer::Overlay,
        "activate-linux".to_owned(),
        qh,
        LayerSurfaceData { index },
    );

    let (width, height) = watermark_size();
    layer_surface.set_size(width as u32, height as u32);
    layer_surface.set_anchor(Anchor::Right | Anchor::Bottom);
    layer_surface.set_margin(0, RIGHT_MARGIN, BOTTOM_MARGIN, 0);
    layer_surface.set_exclusive_zone(-1);
    layer_surface.set_keyboard_interactivity(KeyboardInteractivity::None);

    let empty_input = app.compositor.create_region(qh, ());
    surface.set_input_region(Some(&empty_input));
    empty_input.destroy();

    surface.commit();

    app.overlays.push(Overlay {
        surface,
        _layer_surface: layer_surface,
        buffer: None,
        _shm_file: None,
    });
}

fn watermark_size() -> (i32, i32) {
    let surface =
        ImageSurface::create(Format::ARgb32, 1, 1).expect("create cairo measurement surface");
    let cr = Context::new(&surface).expect("create cairo context");
    let title = text_extents(&cr, "Activate Linux", 22.0);
    let subtitle = text_extents(&cr, "Go to Settings to activate Linux", 14.0);

    let width = title.0.max(subtitle.0).ceil() as i32;
    let height = (title.1 + LINE_GAP + subtitle.1).ceil() as i32;
    (width.max(1), height.max(1))
}

fn text_extents(cr: &Context, text: &str, point_size: f64) -> (f64, f64) {
    cr.select_font_face("Sans", FontSlant::Normal, FontWeight::Normal);
    cr.set_font_size(points_to_pixels(point_size));
    let extents = cr.text_extents(text).expect("measure text");
    (extents.x_advance(), extents.height())
}

fn render_watermark() -> Result<RenderedWatermark, Box<dyn std::error::Error>> {
    let (width, height) = watermark_size();
    let mut surface = ImageSurface::create(Format::ARgb32, width, height)?;
    let cr = Context::new(&surface)?;

    cr.set_operator(Operator::Clear);
    cr.paint()?;
    cr.set_operator(Operator::Over);
    cr.set_source_rgba(1.0, 1.0, 1.0, TEXT_ALPHA);

    cr.select_font_face("Sans", FontSlant::Normal, FontWeight::Normal);
    cr.set_font_size(points_to_pixels(22.0));
    let title = cr.text_extents("Activate Linux")?;
    cr.move_to(-title.x_bearing(), -title.y_bearing());
    cr.show_text("Activate Linux")?;

    cr.set_font_size(points_to_pixels(14.0));
    let subtitle = cr.text_extents("Go to Settings to activate Linux")?;
    cr.move_to(
        -subtitle.x_bearing(),
        title.height() + LINE_GAP - subtitle.y_bearing(),
    );
    cr.show_text("Go to Settings to activate Linux")?;

    surface.flush();
    let stride = surface.stride();
    let pixels = surface.data()?.to_vec();

    Ok(RenderedWatermark {
        width,
        height,
        stride,
        pixels,
    })
}

fn points_to_pixels(points: f64) -> f64 {
    points * 96.0 / 72.0
}

fn draw_overlay(
    app: &mut App,
    index: usize,
    qh: &QueueHandle<App>,
) -> Result<(), Box<dyn std::error::Error>> {
    let rendered = render_watermark()?;
    let size = rendered.pixels.len() as i32;
    let mut file = create_shm_file("activate-linux-watermark")?;
    file.set_len(size as u64)?;
    file.seek(SeekFrom::Start(0))?;
    file.write_all(&rendered.pixels)?;

    let pool = app.shm.create_pool(file.as_fd(), size, qh, ());
    let buffer = pool.create_buffer(
        0,
        rendered.width,
        rendered.height,
        rendered.stride,
        wayland_client::protocol::wl_shm::Format::Argb8888,
        qh,
        (),
    );
    pool.destroy();

    let overlay = &mut app.overlays[index];
    overlay.surface.attach(Some(&buffer), 0, 0);
    overlay
        .surface
        .damage_buffer(0, 0, rendered.width, rendered.height);
    overlay.surface.commit();
    overlay.buffer = Some(buffer);
    overlay._shm_file = Some(file);

    Ok(())
}

fn create_shm_file(name: &str) -> Result<File, Box<dyn std::error::Error>> {
    let cname = std::ffi::CString::new(name)?;
    let fd = unsafe { libc::memfd_create(cname.as_ptr(), libc::MFD_CLOEXEC) };
    if fd < 0 {
        return Err(std::io::Error::last_os_error().into());
    }

    Ok(unsafe { File::from_raw_fd(fd) })
}

impl Dispatch<ZwlrLayerSurfaceV1, LayerSurfaceData> for App {
    fn event(
        app: &mut Self,
        layer_surface: &ZwlrLayerSurfaceV1,
        event: LayerSurfaceEvent,
        data: &LayerSurfaceData,
        _conn: &Connection,
        qh: &QueueHandle<Self>,
    ) {
        match event {
            LayerSurfaceEvent::Configure { serial, .. } => {
                layer_surface.ack_configure(serial);
                if let Err(err) = draw_overlay(app, data.index, qh) {
                    eprintln!("activate-linux: failed to draw overlay: {err}");
                }
            }
            LayerSurfaceEvent::Closed => process::exit(0),
            _ => {}
        }
    }
}

impl Dispatch<WlRegistry, GlobalListContents> for App {
    fn event(
        _app: &mut Self,
        _registry: &WlRegistry,
        _event: <WlRegistry as Proxy>::Event,
        _data: &GlobalListContents,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
    ) {
    }
}

delegate_noop!(App: ignore WlCompositor);
delegate_noop!(App: ignore WlShm);
delegate_noop!(App: ignore WlShmPool);
delegate_noop!(App: ignore WlBuffer);
delegate_noop!(App: ignore WlSurface);
delegate_noop!(App: ignore WlRegion);
delegate_noop!(App: ignore WlOutput);
delegate_noop!(App: ignore WlCallback);
delegate_noop!(App: ignore ZwlrLayerShellV1);
