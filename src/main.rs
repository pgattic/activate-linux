use std::{
    env,
    fs::File,
    io::Write,
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
const LINE_GAP: f64 = 16.0;
const DEFAULT_LINE1: &str = "Activate Linux";
const DEFAULT_LINE2: &str = "Go to Settings to activate Linux.";
const LINE1_FONT_SIZE: f64 = 22.0;
const LINE2_FONT_SIZE: f64 = 16.0;

struct App {
    compositor: WlCompositor,
    buffer: WlBuffer,
    width: i32,
    height: i32,
    overlays: Vec<Overlay>,
}

struct WatermarkText {
    line1: String,
    line2: String,
}

struct Overlay {
    surface: WlSurface,
    _layer_surface: ZwlrLayerSurfaceV1,
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
    let text = parse_args()?;
    let conn = Connection::connect_to_env()?;
    let (globals, mut event_queue) = registry_queue_init::<App>(&conn)?;
    let qh = event_queue.handle();

    let compositor = bind::<WlCompositor>(&globals, &qh, 4..=6)?;
    let shm = bind::<WlShm>(&globals, &qh, 1..=1)?;
    let layer_shell = bind::<ZwlrLayerShellV1>(&globals, &qh, 1..=4)?;
    let rendered = render_watermark(&text)?;
    let width = rendered.width;
    let height = rendered.height;
    let buffer = create_shm_buffer(&shm, rendered, &qh)?;
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
        buffer,
        width,
        height,
        overlays: Vec::with_capacity(outputs.len()),
    };

    for output in outputs {
        create_overlay(&mut app, &layer_shell, &output, &qh)?;
    }

    loop {
        event_queue.blocking_dispatch(&mut app)?;
    }
}

fn parse_args() -> Result<WatermarkText, Box<dyn std::error::Error>> {
    let mut args = env::args().skip(1);
    let Some(line1) = args.next() else {
        return Ok(WatermarkText {
            line1: DEFAULT_LINE1.to_owned(),
            line2: DEFAULT_LINE2.to_owned(),
        });
    };

    if line1 == "-h" || line1 == "--help" {
        print_usage();
        process::exit(0);
    }

    let line2 = args.next().unwrap_or_else(|| DEFAULT_LINE2.to_owned());
    if args.next().is_some() {
        return Err("usage: activate-linux [line1 [line2]]".into());
    }

    Ok(WatermarkText { line1, line2 })
}

fn print_usage() {
    println!(
        "usage: activate-linux [line1 [line2]]\n\nDefaults:\n  line1: {DEFAULT_LINE1}\n  line2: {DEFAULT_LINE2}"
    );
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
) -> Result<(), Box<dyn std::error::Error>> {
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

    layer_surface.set_size(app.width as u32, app.height as u32);
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
    });

    Ok(())
}

fn watermark_size(text: &WatermarkText) -> Result<(i32, i32), Box<dyn std::error::Error>> {
    let surface = ImageSurface::create(Format::ARgb32, 1, 1)?;
    let cr = Context::new(&surface)?;
    let title = text_extents(&cr, &text.line1, LINE1_FONT_SIZE)?;
    let subtitle = text_extents(&cr, &text.line2, LINE2_FONT_SIZE)?;

    let width = title.0.max(subtitle.0).ceil() as i32;
    let height = (title.1 + LINE_GAP + subtitle.1).ceil() as i32;
    Ok((width.max(1), height.max(1)))
}

fn text_extents(
    cr: &Context,
    text: &str,
    point_size: f64,
) -> Result<(f64, f64), Box<dyn std::error::Error>> {
    cr.select_font_face("Sans", FontSlant::Normal, FontWeight::Normal);
    cr.set_font_size(points_to_pixels(point_size));
    let extents = cr.text_extents(text)?;
    Ok((extents.x_advance(), extents.height()))
}

fn render_watermark(text: &WatermarkText) -> Result<RenderedWatermark, Box<dyn std::error::Error>> {
    let (width, height) = watermark_size(text)?;
    let mut surface = ImageSurface::create(Format::ARgb32, width, height)?;

    {
        let cr = Context::new(&surface)?;

        cr.set_operator(Operator::Clear);
        cr.paint()?;
        cr.set_operator(Operator::Over);
        cr.set_source_rgba(1.0, 1.0, 1.0, TEXT_ALPHA);

        cr.select_font_face("Sans", FontSlant::Normal, FontWeight::Normal);
        cr.set_font_size(points_to_pixels(LINE1_FONT_SIZE));
        let title = cr.text_extents(&text.line1)?;
        cr.move_to(-title.x_bearing(), -title.y_bearing());
        cr.show_text(&text.line1)?;

        cr.set_font_size(points_to_pixels(LINE2_FONT_SIZE));
        let subtitle = cr.text_extents(&text.line2)?;
        cr.move_to(
            -subtitle.x_bearing(),
            title.height() + LINE_GAP - subtitle.y_bearing(),
        );
        cr.show_text(&text.line2)?;
    }

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

fn draw_overlay(app: &mut App, index: usize) -> Result<(), Box<dyn std::error::Error>> {
    let overlay = &mut app.overlays[index];
    overlay.surface.attach(Some(&app.buffer), 0, 0);
    overlay.surface.damage_buffer(0, 0, app.width, app.height);
    overlay.surface.commit();

    Ok(())
}

fn create_shm_buffer(
    shm: &WlShm,
    rendered: RenderedWatermark,
    qh: &QueueHandle<App>,
) -> Result<WlBuffer, Box<dyn std::error::Error>> {
    let size = rendered.pixels.len() as i32;
    let mut file = create_shm_file("activate-linux-watermark")?;
    file.set_len(size as u64)?;
    file.write_all(&rendered.pixels)?;

    let pool = shm.create_pool(file.as_fd(), size, qh, ());
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

    Ok(buffer)
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
        _qh: &QueueHandle<Self>,
    ) {
        match event {
            LayerSurfaceEvent::Configure { serial, .. } => {
                layer_surface.ack_configure(serial);
                if let Err(err) = draw_overlay(app, data.index) {
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
