use std::{
    fs::File,
    io::Write,
    os::fd::{AsFd, FromRawFd},
    process,
};

use cairo::{Context, FontSlant, FontWeight, Format, ImageSurface, Operator};
use clap::{Parser, ValueEnum};
use wayland_client::{
    delegate_noop,
    globals::{registry_queue_init, BindError, GlobalListContents},
    protocol::{
        wl_buffer::WlBuffer,
        wl_callback::WlCallback,
        wl_compositor::WlCompositor,
        wl_output::{Event as OutputEvent, WlOutput},
        wl_region::WlRegion,
        wl_registry::WlRegistry,
        wl_shm::WlShm,
        wl_shm_pool::WlShmPool,
        wl_surface::WlSurface,
    },
    Connection, Dispatch, Proxy, QueueHandle,
};
use wayland_protocols_wlr::layer_shell::v1::client::{
    zwlr_layer_shell_v1::{Layer, ZwlrLayerShellV1},
    zwlr_layer_surface_v1::{
        Anchor, Event as LayerSurfaceEvent, KeyboardInteractivity, ZwlrLayerSurfaceV1,
    },
};

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

const DEFAULT_OPACITY: f64 = 0.35;
const DEFAULT_HORIZONTAL_MARGIN: i32 = 80;
const DEFAULT_VERTICAL_MARGIN: i32 = 110 - 60; // Windows taskbar is 60px
const DEFAULT_TOP_MARGIN: i32 = DEFAULT_VERTICAL_MARGIN;
const DEFAULT_LEFT_MARGIN: i32 = DEFAULT_HORIZONTAL_MARGIN;
const DEFAULT_RIGHT_MARGIN: i32 = DEFAULT_HORIZONTAL_MARGIN;
const DEFAULT_BOTTOM_MARGIN: i32 = DEFAULT_VERTICAL_MARGIN;
const LINE_GAP: f64 = 16.0;
const DEFAULT_LINE1: &str = "Activate Linux";
const DEFAULT_LINE2: &str = "Go to Settings to activate Linux.";
const LINE1_FONT_SIZE: f64 = 16.5;
const LINE2_FONT_SIZE: f64 = 12.1;
const RASTER_PADDING: i32 = 4;

struct App {
    compositor: WlCompositor,
    shm: WlShm,
    options: Options,
    logical_width: i32,
    logical_height: i32,
    overlays: Vec<Overlay>,
}

#[derive(Parser)]
#[command(version, about)]
struct Cli {
    /// Corner to place the watermark in.
    #[arg(long, value_enum, default_value_t = Corner::BottomRight)]
    corner: Corner,

    /// Set all edge margins, in logical pixels.
    #[arg(long, value_name = "PX")]
    margin: Option<i32>,

    /// Top margin, in logical pixels.
    #[arg(long, value_name = "PX")]
    margin_top: Option<i32>,

    /// Right margin, in logical pixels.
    #[arg(long, value_name = "PX")]
    margin_right: Option<i32>,

    /// Bottom margin, in logical pixels.
    #[arg(long, value_name = "PX")]
    margin_bottom: Option<i32>,

    /// Left margin, in logical pixels.
    #[arg(long, value_name = "PX")]
    margin_left: Option<i32>,

    /// Text color as #RGB, #RRGGBB, RGB, or RRGGBB.
    #[arg(long, default_value = "ffffff", value_parser = parse_color)]
    color: Color,

    /// Text opacity from 0.0 to 1.0.
    #[arg(long, default_value_t = DEFAULT_OPACITY, value_parser = parse_opacity)]
    opacity: f64,

    /// First line of text.
    #[arg(default_value = DEFAULT_LINE1)]
    line1: String,

    /// Second line of text.
    #[arg(default_value = DEFAULT_LINE2)]
    line2: String,
}

#[derive(Clone, Copy, ValueEnum)]
enum Corner {
    TopLeft,
    TopRight,
    BottomLeft,
    BottomRight,
}

struct Options {
    text: WatermarkText,
    corner: Corner,
    margins: Margins,
    color: Color,
    opacity: f64,
}

struct WatermarkText {
    line1: String,
    line2: String,
}

struct Margins {
    top: i32,
    right: i32,
    bottom: i32,
    left: i32,
}

#[derive(Clone, Copy)]
struct Color {
    red: f64,
    green: f64,
    blue: f64,
}

impl Corner {
    fn anchor(self) -> Anchor {
        match self {
            Self::TopLeft => Anchor::Top | Anchor::Left,
            Self::TopRight => Anchor::Top | Anchor::Right,
            Self::BottomLeft => Anchor::Bottom | Anchor::Left,
            Self::BottomRight => Anchor::Bottom | Anchor::Right,
        }
    }

    fn layer_margins(self, margins: &Margins) -> Margins {
        match self {
            Self::TopLeft => Margins {
                top: margins.top,
                right: 0,
                bottom: 0,
                left: margins.left,
            },
            Self::TopRight => Margins {
                top: margins.top,
                right: margins.right,
                bottom: 0,
                left: 0,
            },
            Self::BottomLeft => Margins {
                top: 0,
                right: 0,
                bottom: margins.bottom,
                left: margins.left,
            },
            Self::BottomRight => Margins {
                top: 0,
                right: margins.right,
                bottom: margins.bottom,
                left: 0,
            },
        }
    }
}

struct Overlay {
    surface: WlSurface,
    _layer_surface: ZwlrLayerSurfaceV1,
    scale: i32,
    configured: bool,
    buffer: Option<WlBuffer>,
    _shm_file: Option<File>,
}

struct LayerSurfaceData {
    index: usize,
}

struct OutputData {
    index: usize,
}

struct RenderedWatermark {
    buffer_width: i32,
    buffer_height: i32,
    stride: i32,
    pixels: Vec<u8>,
}

#[derive(Clone, Copy)]
struct LineExtents {
    x_bearing: f64,
    y_bearing: f64,
    width: f64,
    height: f64,
}

struct WatermarkLayout {
    width: i32,
    height: i32,
    title: LineExtents,
    subtitle: LineExtents,
}

impl WatermarkLayout {
    fn measure(text: &WatermarkText) -> Result<Self> {
        let surface = ImageSurface::create(Format::ARgb32, 1, 1)?;
        let cr = Context::new(&surface)?;
        let title = line_extents(&cr, &text.line1, LINE1_FONT_SIZE)?;
        let subtitle = line_extents(&cr, &text.line2, LINE2_FONT_SIZE)?;
        let padding = f64::from(RASTER_PADDING) * 2.0;

        let width = (title.width.max(subtitle.width) + padding).ceil() as i32;
        let height = (title.height + LINE_GAP + subtitle.height + padding).ceil() as i32;

        Ok(Self {
            width: width.max(1),
            height: height.max(1),
            title,
            subtitle,
        })
    }
}

fn main() {
    if let Err(err) = run() {
        eprintln!("activate-linux: {err}");
        process::exit(1);
    }
}

fn run() -> Result<()> {
    let options = Options::from(Cli::parse());
    let conn = Connection::connect_to_env()?;
    let (globals, mut event_queue) = registry_queue_init::<App>(&conn)?;
    let qh = event_queue.handle();

    let compositor = bind::<WlCompositor>(&globals, &qh, 4..=6)?;
    let shm = bind::<WlShm>(&globals, &qh, 1..=1)?;
    let layer_shell = bind::<ZwlrLayerShellV1>(&globals, &qh, 1..=4)?;
    let layout = WatermarkLayout::measure(&options.text)?;
    let outputs = globals
        .contents()
        .clone_list()
        .into_iter()
        .filter(|global| global.interface == WlOutput::interface().name)
        .enumerate()
        .map(|(index, global)| {
            let version = global.version.min(WlOutput::interface().version);
            globals.bind::<WlOutput, _, _>(&qh, version..=version, OutputData { index })
        })
        .collect::<std::result::Result<Vec<_>, BindError>>()?;

    if outputs.is_empty() {
        return Err("compositor did not advertise any wl_output globals".into());
    }

    let mut app = App {
        compositor,
        shm,
        options,
        logical_width: layout.width,
        logical_height: layout.height,
        overlays: Vec::with_capacity(outputs.len()),
    };

    for output in outputs {
        create_overlay(&mut app, &layer_shell, &output, &qh);
    }

    loop {
        event_queue.blocking_dispatch(&mut app)?;
    }
}

impl From<Cli> for Options {
    fn from(cli: Cli) -> Self {
        let margin = cli.margin;
        Self {
            text: WatermarkText {
                line1: cli.line1,
                line2: cli.line2,
            },
            corner: cli.corner,
            margins: Margins {
                top: cli.margin_top.or(margin).unwrap_or(DEFAULT_TOP_MARGIN),
                right: cli.margin_right.or(margin).unwrap_or(DEFAULT_RIGHT_MARGIN),
                bottom: cli
                    .margin_bottom
                    .or(margin)
                    .unwrap_or(DEFAULT_BOTTOM_MARGIN),
                left: cli.margin_left.or(margin).unwrap_or(DEFAULT_LEFT_MARGIN),
            },
            color: cli.color,
            opacity: cli.opacity,
        }
    }
}

fn parse_color(input: &str) -> std::result::Result<Color, String> {
    let hex = input.strip_prefix('#').unwrap_or(input);
    let expanded;
    let hex = match hex.len() {
        3 => {
            expanded = hex.chars().flat_map(|ch| [ch, ch]).collect::<String>();
            expanded.as_str()
        }
        6 => hex,
        _ => return Err("expected #RGB, #RRGGBB, RGB, or RRGGBB".to_owned()),
    };

    let value = u32::from_str_radix(hex, 16).map_err(|_| "invalid hex color".to_owned())?;
    Ok(Color {
        red: f64::from((value >> 16) & 0xff) / 255.0,
        green: f64::from((value >> 8) & 0xff) / 255.0,
        blue: f64::from(value & 0xff) / 255.0,
    })
}

fn parse_opacity(input: &str) -> std::result::Result<f64, String> {
    let opacity = input
        .parse::<f64>()
        .map_err(|_| "opacity must be a number from 0.0 to 1.0".to_owned())?;
    if (0.0..=1.0).contains(&opacity) {
        Ok(opacity)
    } else {
        Err("opacity must be from 0.0 to 1.0".to_owned())
    }
}

fn visual_margin(margin: i32) -> i32 {
    margin - RASTER_PADDING
}

fn bind<I>(
    globals: &wayland_client::globals::GlobalList,
    qh: &QueueHandle<App>,
    version: std::ops::RangeInclusive<u32>,
) -> std::result::Result<I, BindError>
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

    layer_surface.set_size(app.logical_width as u32, app.logical_height as u32);
    layer_surface.set_anchor(app.options.corner.anchor());
    let margins = app.options.corner.layer_margins(&app.options.margins);
    layer_surface.set_margin(
        visual_margin(margins.top),
        visual_margin(margins.right),
        visual_margin(margins.bottom),
        visual_margin(margins.left),
    );
    layer_surface.set_exclusive_zone(-1);
    layer_surface.set_keyboard_interactivity(KeyboardInteractivity::None);

    let empty_input = app.compositor.create_region(qh, ());
    surface.set_input_region(Some(&empty_input));
    empty_input.destroy();

    surface.commit();

    app.overlays.push(Overlay {
        surface,
        _layer_surface: layer_surface,
        scale: 1,
        configured: false,
        buffer: None,
        _shm_file: None,
    });
}

fn line_extents(cr: &Context, text: &str, point_size: f64) -> Result<LineExtents> {
    cr.select_font_face("Sans", FontSlant::Normal, FontWeight::Normal);
    cr.set_font_size(points_to_pixels(point_size));
    let extents = cr.text_extents(text)?;
    Ok(LineExtents {
        x_bearing: extents.x_bearing(),
        y_bearing: extents.y_bearing(),
        width: extents.width(),
        height: extents.height(),
    })
}

fn render_watermark(options: &Options, scale: i32) -> Result<RenderedWatermark> {
    let text = &options.text;
    let layout = WatermarkLayout::measure(text)?;
    let logical_width = layout.width;
    let logical_height = layout.height;
    let buffer_width = logical_width * scale;
    let buffer_height = logical_height * scale;
    let mut surface = ImageSurface::create(Format::ARgb32, buffer_width, buffer_height)?;

    {
        let cr = Context::new(&surface)?;

        cr.set_operator(Operator::Clear);
        cr.paint()?;
        cr.set_operator(Operator::Over);
        cr.set_source_rgba(
            options.color.red,
            options.color.green,
            options.color.blue,
            options.opacity,
        );
        cr.scale(scale as f64, scale as f64);

        cr.select_font_face("Sans", FontSlant::Normal, FontWeight::Normal);
        cr.set_font_size(points_to_pixels(LINE1_FONT_SIZE));
        cr.move_to(
            f64::from(RASTER_PADDING) - layout.title.x_bearing,
            f64::from(RASTER_PADDING) - layout.title.y_bearing,
        );
        cr.show_text(&text.line1)?;

        cr.set_font_size(points_to_pixels(LINE2_FONT_SIZE));
        cr.move_to(
            f64::from(RASTER_PADDING) - layout.subtitle.x_bearing,
            f64::from(RASTER_PADDING) + layout.title.height + LINE_GAP - layout.subtitle.y_bearing,
        );
        cr.show_text(&text.line2)?;
    }

    surface.flush();
    let stride = surface.stride();
    let pixels = surface.data()?.to_vec();

    Ok(RenderedWatermark {
        buffer_width,
        buffer_height,
        stride,
        pixels,
    })
}

fn points_to_pixels(points: f64) -> f64 {
    points * 96.0 / 72.0
}

fn draw_overlay(app: &mut App, index: usize, qh: &QueueHandle<App>) -> Result<()> {
    let scale = app.overlays[index].scale;
    let rendered = render_watermark(&app.options, scale)?;
    let (buffer, file) = create_shm_buffer(&app.shm, rendered, qh)?;
    let overlay = &mut app.overlays[index];
    overlay.surface.set_buffer_scale(scale);
    overlay.surface.attach(Some(&buffer), 0, 0);
    overlay
        .surface
        .damage_buffer(0, 0, app.logical_width * scale, app.logical_height * scale);
    overlay.surface.commit();
    overlay.buffer = Some(buffer);
    overlay._shm_file = Some(file);

    Ok(())
}

fn create_shm_buffer(
    shm: &WlShm,
    rendered: RenderedWatermark,
    qh: &QueueHandle<App>,
) -> Result<(WlBuffer, File)> {
    let size = rendered.pixels.len() as i32;
    let mut file = create_shm_file("activate-linux-watermark")?;
    file.set_len(size as u64)?;
    file.write_all(&rendered.pixels)?;

    let pool = shm.create_pool(file.as_fd(), size, qh, ());
    let buffer = pool.create_buffer(
        0,
        rendered.buffer_width,
        rendered.buffer_height,
        rendered.stride,
        wayland_client::protocol::wl_shm::Format::Argb8888,
        qh,
        (),
    );
    pool.destroy();

    Ok((buffer, file))
}

fn create_shm_file(name: &str) -> Result<File> {
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
                app.overlays[data.index].configured = true;
                if let Err(err) = draw_overlay(app, data.index, qh) {
                    eprintln!("activate-linux: failed to draw overlay: {err}");
                }
            }
            LayerSurfaceEvent::Closed => process::exit(0),
            _ => {}
        }
    }
}

impl Dispatch<WlOutput, OutputData> for App {
    fn event(
        app: &mut Self,
        _output: &WlOutput,
        event: OutputEvent,
        data: &OutputData,
        _conn: &Connection,
        qh: &QueueHandle<Self>,
    ) {
        let OutputEvent::Scale { factor } = event else {
            return;
        };

        let scale = factor.max(1);
        let Some(overlay) = app.overlays.get_mut(data.index) else {
            return;
        };
        if overlay.scale == scale {
            return;
        }

        overlay.scale = scale;
        if !overlay.configured {
            return;
        }

        if let Err(err) = draw_overlay(app, data.index, qh) {
            eprintln!("activate-linux: failed to redraw scaled overlay: {err}");
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
delegate_noop!(App: ignore WlCallback);
delegate_noop!(App: ignore ZwlrLayerShellV1);
