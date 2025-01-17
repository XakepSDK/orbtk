//! Wrapper consuming tinyskia functions.

use smallvec::SmallVec;
use std::{
    collections::HashMap,
    f64::consts::{FRAC_PI_2, PI},
    ptr,
};
use tiny_skia::{
    ClipMask, FillRule, Paint, PathBuilder, Pixmap, PixmapPaint, Shader, Stroke, Transform,
};

use crate::{common::*, utils::*, PipelineTrait, RenderConfig, RenderTarget, TextMetrics};

pub use self::font::*;
pub use self::image::Image;

mod font;
mod image;

#[derive(Debug)]
struct State {
    config: RenderConfig,
    path_rect: PathRect,
    clips_count: usize,
    //clip_mask: ClipMask,
    transform: Transform,
}

type StatesOnStack = [State; 2];

/// The RenderContext2D trait, provides the rendering context (`ctx`). It is used
/// for drawing shapes, text, images, and other objects.
pub struct RenderContext2D {
    background: Color,
    clips_count: usize,
    //clip_mask: ClipMask,
    config: RenderConfig,
    fill_paint: Paint<'static>,
    fonts: HashMap<String, Font>,
    path_builder: PathBuilder,
    path_rect: PathRect,
    pixmap: Pixmap,
    saved_states: SmallVec<StatesOnStack>,
    stroke_paint: Paint<'static>,
    transform: Transform,
}

impl RenderContext2D {
    /// Creates a new 2d render context.
    pub fn new(width: f64, height: f64) -> Self {
        let pixmap = Pixmap::new(width as u32, height as u32).unwrap();
        RenderContext2D {
            background: Color::default(),
            clips_count: 0,
            //clip_mask: ClipMask::default(),
            config: RenderConfig::default(),
            fill_paint: Self::paint_from_brush(
                &Brush::default(),
                Rectangle::new(Point::new(0.0, 0.0), Size::new(0.0, 0.0)),
                1.0,
            ),
            fonts: HashMap::new(),
            path_builder: PathBuilder::new(),
            path_rect: PathRect::new(None),
            pixmap,
            saved_states: SmallVec::<StatesOnStack>::new(),
            stroke_paint: Self::paint_from_brush(
                &Brush::default(),
                Rectangle::new(Point::new(0.0, 0.0), Size::new(0.0, 0.0)),
                1.0,
            ),
            transform: Transform::identity(),
        }
    }

    fn paint_from_brush(brush: &Brush, frame: Rectangle, global_alpha: f32) -> Paint<'static> {
        let shader = match brush {
            Brush::SolidColor(color) => {
                let mut color =
                    tiny_skia::Color::from_rgba8(color.b(), color.g(), color.r(), color.a());
                color.set_alpha(color.alpha() * global_alpha);
                Shader::SolidColor(color)
            }
            Brush::Gradient(Gradient {
                kind: GradientKind::Linear(coords),
                stops,
                repeat,
            }) => {
                let spread = match repeat {
                    true => tiny_skia::SpreadMode::Repeat,
                    false => tiny_skia::SpreadMode::Pad,
                };
                let (start, end) = match coords {
                    LinearGradientCoords::Ends { start, end } => {
                        (*start + frame.position(), *end + frame.position())
                    }
                    LinearGradientCoords::Angle {
                        angle,
                        displacement,
                    } => {
                        let z = linear_gradient_ends_from_angle(*angle, frame.size());
                        let disp = displacement.pixels(frame.size());
                        let start = frame.position() + frame.size() / 2.0 + -z + disp;
                        let end = frame.position() + frame.size() / 2.0 + z + disp;
                        (start, end)
                    }
                    LinearGradientCoords::Direction {
                        direction,
                        displacement,
                    } => {
                        let width = frame.width();
                        let height = frame.height();
                        let (mut start, mut end) = direction.cross(width, height);
                        let displacement = displacement.pixels(frame.size());
                        start = start + frame.position() + displacement;
                        end = end + frame.position() + displacement;
                        (start, end)
                    }
                };
                let g_stops = build_unit_percent_gradient(stops, end.distance(start), |p, c| {
                    let mut color = tiny_skia::Color::from_rgba8(c.b(), c.g(), c.r(), c.a());
                    color.set_alpha(color.alpha() * global_alpha);
                    tiny_skia::GradientStop::new(p as f32, color)
                });
                let tstart = tiny_skia::Point::from_xy(start.x() as f32, start.y() as f32);
                let tend = tiny_skia::Point::from_xy(end.x() as f32, end.y() as f32);
                tiny_skia::LinearGradient::new(
                    tstart,
                    tend,
                    g_stops,
                    spread,
                    tiny_skia::Transform::identity(),
                )
                .unwrap_or(Shader::SolidColor(tiny_skia::Color::WHITE))
            }
        };
        Paint {
            shader,
            anti_alias: true,
            ..Default::default()
        }
    }

    /// Registers a new font file.
    pub fn register_font(&mut self, family: &str, font_file: &'static [u8]) {
        if self.fonts.contains_key(family) {
            return;
        }

        if let Ok(font) = Font::from_bytes(font_file) {
            self.fonts.insert(family.to_string(), font);
        }
    }

    /// Resizes pixmap with new height and width
    pub fn resize(&mut self, width: f64, height: f64) {
        self.pixmap = Pixmap::new(width as u32, height as u32).unwrap();
    }

    /// Set the background of the render context.
    pub fn set_background(&mut self, background: Color) {
        self.background = background;
    }

    // Rectangles

    /// Draws a filled rectangle whose starting point is at the
    /// coordinates {x, y} with the specified width and height and
    /// whose style is determined by the fillStyle attribute.
    pub fn fill_rect(&mut self, x: f64, y: f64, width: f64, height: f64) {
        if width > 0.0 && height > 0.0 {
            self.path_rect.record_rect(x, y, width, height);
            let rect = self.path_rect.get_rect().unwrap();
            self.fill_paint =
                Self::paint_from_brush(&self.config.fill_style, rect, self.config.alpha as f32);
            self.pixmap.fill_rect(
                tiny_skia::Rect::from_xywh(
                    (x as f32).floor(),
                    (y as f32).floor(),
                    width as f32,
                    height as f32,
                )
                .unwrap(),
                &self.fill_paint,
                tiny_skia::Transform::identity(),
                None,
            );
        }
    }

    /// Draws a rectangle that is stroked (outlined) according to the
    /// current strokeStyle and other ctx settings.
    pub fn stroke_rect(&mut self, x: f64, y: f64, width: f64, height: f64) {
        self.rect(x, y, width, height);
        self.stroke();
    }

    // Text

    /// Intern function to draw an arc segment minor or equal to 90°
    #[allow(clippy::many_single_char_names)]
    fn arc_fragment(&mut self, x: f64, y: f64, radius: f64, start_angle: f64, end_angle: f64) {
        let (end_sin, end_cos) = end_angle.sin_cos();
        let end_x = x + end_cos * radius;
        let end_y = y + end_sin * radius;
        let (start_sin, start_cos) = start_angle.sin_cos();
        let start_x = x + start_cos * radius;
        let start_y = y + start_sin * radius;

        let t1x = y - start_y;
        let t1y = start_x - x;
        let t2x = end_y - y;
        let t2y = x - end_x;
        let dx = (start_x + end_x) / 2.0 - x;
        let dy = (start_y + end_y) / 2.0 - y;
        let tx = 3.0 / 8.0 * (t1x + t2x);
        let ty = 3.0 / 8.0 * (t1y + t2y);
        let a = tx * tx + ty * ty;
        let b = dx * tx + dy * ty;
        let c = dx * dx + dy * dy - radius * radius;

        let bb = b;
        let d = (b * bb) - a * c;
        if d > 0.0 {
            let k = (d.sqrt() - b) / a;
            self.path_builder.cubic_to(
                (start_x + k * t1x) as f32,
                (start_y + k * t1y) as f32,
                (end_x + k * t2x) as f32,
                (end_y + k * t2y) as f32,
                (x + end_cos * radius) as f32,
                (y + end_sin * radius) as f32,
            );
        }
    }

    /// Creates a circular arc centered at (x, y) with a radius of
    /// given `radius` value. The path starts at value `startAngle`
    /// and ends at value `endAngle`.
    pub fn arc(&mut self, x: f64, y: f64, radius: f64, mut start_angle: f64, mut end_angle: f64) {
        self.path_rect
            .record_arc(x, y, radius, start_angle, end_angle);
        if start_angle.is_sign_negative() {
            start_angle = TAU - -start_angle;
        }
        if end_angle.is_sign_negative() {
            end_angle = TAU - -end_angle;
        }
        let premult_k = 0.552284749831 * radius;
        let (start_sin, start_cos) = start_angle.sin_cos();
        if end_angle - start_angle < TAU {
            self.path_builder.move_to(x as f32, y as f32);
            self.path_builder.line_to(
                (x + start_cos * radius) as f32,
                (y + start_sin * radius) as f32,
            );
        } else {
            self.path_builder.move_to(
                (x + start_cos * radius) as f32,
                (y + start_sin * radius) as f32,
            );
        }
        if end_angle - start_angle < FRAC_PI_2 {
            self.arc_fragment(x, y, radius, start_angle, end_angle);
            self.path_builder.line_to(x as f32, y as f32);
            return;
        } else if start_angle % FRAC_PI_2 > f64::EPSILON {
            self.arc_fragment(
                x,
                y,
                radius,
                start_angle,
                start_angle + FRAC_PI_2 - start_angle % FRAC_PI_2,
            );
        }
        // Build the four arc quadrants if they are in the range between start_angle and end_angle
        if start_angle <= 0.0 && end_angle >= FRAC_PI_2 {
            self.path_builder.cubic_to(
                (x + radius) as f32,
                (y + premult_k) as f32,
                (x + premult_k) as f32,
                (y + radius) as f32,
                x as f32,
                (y + radius) as f32,
            );
        }
        if start_angle <= FRAC_PI_2 && end_angle >= PI {
            self.path_builder.cubic_to(
                (x - premult_k) as f32,
                (y + radius) as f32,
                (x - radius) as f32,
                (y + premult_k) as f32,
                (x - radius) as f32,
                y as f32,
            );
        }
        if start_angle <= PI && end_angle >= PI + FRAC_PI_2 {
            self.path_builder.cubic_to(
                (x - radius) as f32,
                (y - premult_k) as f32,
                (x - premult_k) as f32,
                (y - radius) as f32,
                x as f32,
                (y - radius) as f32,
            );
        }
        if start_angle <= PI + FRAC_PI_2 && end_angle >= TAU {
            self.path_builder.cubic_to(
                (x + premult_k) as f32,
                (y - radius) as f32,
                (x + radius) as f32,
                (y - premult_k) as f32,
                (x + radius) as f32,
                y as f32,
            );
        }
        self.arc_fragment(x, y, radius, end_angle - end_angle % FRAC_PI_2, end_angle);
        if end_angle - start_angle < TAU {
            self.path_builder.line_to(x as f32, y as f32);
        }
    }

    /// Starts a new path by emptying the list of sub-paths. You should call this
    /// method, if you want to create a new path.
    pub fn begin_path(&mut self) {
        self.path_builder = PathBuilder::new();
        self.path_rect.rebirth();
    }

    /// Adds a cubic Bézier curve to the current sub-path.
    /// It requires three points: the first two are control points and
    /// the third one is the end point. The starting point is the
    /// latest point in the current path, which can be changed using
    /// MoveTo{} before creating the Bézier curve.
    pub fn bezier_curve_to(&mut self, cp1x: f64, cp1y: f64, cp2x: f64, cp2y: f64, x: f64, y: f64) {
        self.path_builder.cubic_to(
            cp1x as f32,
            cp1y as f32,
            cp2x as f32,
            cp2y as f32,
            x as f32,
            y as f32,
        );
        self.path_rect
            .record_bezier_curve_to(cp1x, cp1y, cp2x, cp2y, x, y);
    }

    /// Creates a clipping mask applied to the current
    /// paths. Everything drawn after calling clip() will only act
    /// inside the clipping path.
    pub fn clip(&mut self) {
        if let Some(clip_path) = self.path_builder.clone().finish() {
            let mut clip_mask = ClipMask::new();
            clip_mask.set_path(
                self.pixmap.width() as u32,
                self.pixmap.height() as u32,
                &clip_path,
                FillRule::EvenOdd,
                true,
            );
            //self.clip_mask = clip_mask;
        }
        self.path_rect.record_clip();
        self.clips_count += 1;
    }

    /// When closing a path, the method attempts to add a straight
    /// line starting from the current point to the start point of the
    /// current sub-path. Nothing will happen, if the shape has
    /// already been closed or only a single point is referenced.
    pub fn close_path(&mut self) {
        self.path_builder.close();
        self.path_rect.record_path_close();
    }

    /// Draws the image.
    pub fn draw_image(&mut self, image: &Image, x: f64, y: f64) {
        let mut pixmap = Pixmap::new(image.width() as u32, image.height() as u32).unwrap();
        unsafe {
            ptr::copy_nonoverlapping(
                image.data().as_ptr() as *const u8,
                pixmap.data_mut().as_mut_ptr(),
                image.data().len() * 4,
            )
        }
        self.pixmap.draw_pixmap(
            x as i32,
            y as i32,
            pixmap.as_ref(),
            &PixmapPaint::default(),
            Transform::identity(),
            None,
        );
    }

    /// Draws the pipeline.
    pub fn draw_pipeline(
        &mut self,
        x: f64,
        y: f64,
        width: f64,
        height: f64,
        pipeline: Box<dyn PipelineTrait>,
    ) {
        let mut render_target = RenderTarget::new(width as u32, height as u32);
        pipeline.draw_pipeline(&mut render_target);
        self.draw_render_target(&render_target, x, y);
    }

    /// Draws a render target.
    pub fn draw_render_target(&mut self, render_target: &RenderTarget, x: f64, y: f64) {
        let mut pixmap =
            Pixmap::new(render_target.width() as u32, render_target.height() as u32).unwrap();
        unsafe {
            ptr::copy_nonoverlapping(
                render_target.data().as_ptr() as *const u8,
                pixmap.data_mut().as_mut_ptr(),
                render_target.data().len() * 4,
            )
        };
        self.pixmap.draw_pixmap(
            x as i32,
            y as i32,
            pixmap.as_ref(),
            &PixmapPaint::default(),
            Transform::identity(),
            None,
        );
    }

    /// Fills the current or given path with the current file style.
    pub fn fill(&mut self) {
        let rect = match self.path_rect.get_rect() {
            Some(rect) => rect,
            None => return, // The path is empty, do nothing
        };
        self.fill_paint =
            Self::paint_from_brush(&self.config.fill_style, rect, self.config.alpha as f32);
        if let Some(path) = self.path_builder.clone().finish() {
            self.pixmap.fill_path(
                &path,
                &self.fill_paint,
                FillRule::EvenOdd,
                Transform::identity(),
                None,
            );
        }
    }

    /// Draws (fills) a given text at the given (x, y) position.
    pub fn fill_text(&mut self, text: &str, x: f64, y: f64) {
        if text.is_empty() {
            return;
        }

        // TODO: multiline
        let tm = self.measure_text(text);
        let rect = Rectangle::new(Point::new(x, y), Size::new(tm.width, tm.height));
        self.fill_paint =
            Self::paint_from_brush(&self.config.fill_style, rect, self.config.alpha as f32);

        if let Some(font) = self.fonts.get(&self.config.font_config.family) {
            font.render_text(
                self.config.font_config.font_size,
                &self.fill_paint,
                &mut self.pixmap,
                (x, y),
                text,
            );
        }
    }

    /// Adds a straight line to the current sub-path by connecting the
    /// sub-path's last point to the specified {x, y} coordinates.
    pub fn line_to(&mut self, x: f64, y: f64) {
        self.path_builder.line_to(x as f32, y as f32);
        self.path_rect.record_line_to(x, y);
    }

    /// Returns a TextMetrics object.
    pub fn measure(
        &mut self,
        text: &str,
        font_size: f64,
        family: impl Into<String>,
    ) -> TextMetrics {
        self.set_font_family(family);
        self.set_font_size(font_size);
        self.measure_text(text)
    }

    /// Returns a TextMetrics object.
    pub fn measure_text(&mut self, text: &str) -> TextMetrics {
        let mut text_metrics = TextMetrics::default();

        if text.is_empty() {
            return text_metrics;
        }

        if let Some(font) = self.fonts.get(&self.config.font_config.family) {
            let (width, height) = font.measure_text(text, self.config.font_config.font_size);

            text_metrics.width = width;
            text_metrics.height = height;
        }

        text_metrics
    }

    /// Begins a new sub-path at given `point`. The point is specified
    /// by given {x, y} coordinates.
    pub fn move_to(&mut self, x: f64, y: f64) {
        self.path_builder.move_to(x as f32, y as f32);
        self.path_rect.record_move_to(x, y);
    }

    /// Adds a quadratic Bézier curve to the current sub-path.
    pub fn quadratic_curve_to(&mut self, cpx: f64, cpy: f64, x: f64, y: f64) {
        self.path_builder
            .quad_to(cpx as f32, cpy as f32, x as f32, y as f32);
        self.path_rect.record_quadratic_curve_to(cpx, cpy, x, y);
    }

    /// Strokes {outlines} the current or given path with the current stroke style.
    pub fn stroke(&mut self) {
        let rect = match self.path_rect.get_rect() {
            Some(rect) => rect,
            None => return, // The path is empty, do nothing
        };
        self.stroke_paint =
            Self::paint_from_brush(&self.config.stroke_style, rect, self.config.alpha as f32);
        if let Some(path) = self.path_builder.clone().finish() {
            self.pixmap.stroke_path(
                &path,
                &self.stroke_paint,
                &Stroke {
                    width: self.config.line_width as f32,
                    ..Default::default()
                },
                Transform::identity(),
                None,
            );
        }
    }

    /// Adds a rectangle to the current path.
    pub fn rect(&mut self, x: f64, y: f64, width: f64, height: f64) {
        self.path_builder
            .push_rect(x as f32, y as f32, width as f32, height as f32);
        self.path_rect.record_rect(x, y, width, height);
    }

    // Line styles

    /// Sets the alpha value,
    pub fn set_alpha(&mut self, alpha: f32) {
        self.config.alpha = alpha;
    }

    /// Specifies the font family.
    pub fn set_font_family(&mut self, family: impl Into<String>) {
        self.config.font_config.family = family.into();
    }

    /// Specifies the font size.
    pub fn set_font_size(&mut self, size: f64) {
        self.config.font_config.font_size = size + 4.0;
    }

    /// Sets the thickness of lines.
    pub fn set_line_width(&mut self, line_width: f64) {
        self.config.line_width = line_width;
    }

    // Fill and stroke style

    /// Specifies the fill color to use inside shapes.
    pub fn set_fill_style(&mut self, fill_style: impl Into<Brush>) {
        self.config.fill_style = fill_style.into();
    }

    /// Specifies the fill stroke to use inside shapes.
    pub fn set_stroke_style(&mut self, stroke_style: impl Into<Brush>) {
        self.config.stroke_style = stroke_style.into();
    }

    // Pixmap states

    /// Clear the given `brush`.
    pub fn clear(&mut self, brush: &Brush) {
        if let Brush::SolidColor(color) = brush {
            self.pixmap.fill(tiny_skia::Color::from_rgba8(
                color.b(),
                color.g(),
                color.r(),
                color.a(),
            ));
            return;
        }
        let _paint = Self::paint_from_brush(
            brush,
            Rectangle::new(
                Point::new(0., 0.),
                Size::new(self.pixmap.width() as f64, self.pixmap.height() as f64),
            ),
            1.0,
        );
        self.fill_rect(
            0.,
            0.,
            self.pixmap.width() as f64,
            self.pixmap.height() as f64,
        );
    }

    /// Return the pixmap data lenght as an [u8] reference value.
    ///
    /// Byteorder: RGBA
    pub fn data(&self) -> &[u8] {
        self.pixmap.data()
    }

    /// Return the pixmap data lenght as a mutable [u8] reference value.
    ///
    /// Byteorder: RGBA
    pub fn data_mut(&mut self) -> &mut [u8] {
        self.pixmap.data_mut()
    }

    /// Return the pixmap data lenght as a mutable [u8] reference value.
    ///
    /// Byteorder: RGBA
    pub fn data_u8_mut(&mut self) -> &mut [u8] {
        self.pixmap.data_mut()
    }

    /// Restores the most recently saved canvas state by popping the
    /// top entry in the drawing state stack. If there is no saved
    /// state, this method does nothing.
    pub fn restore(&mut self) {
        if let Some(State {
            config,
            path_rect,
            clips_count: former_clips_count,
            //clip_mask,
            transform,
        }) = self.saved_states.pop()
        {
            self.config = config;
            self.path_rect = path_rect;
            // FIXME
            /*for _ in former_clips_count.self.clips_count {
                self.pixmap.pop_clip();
            }*/
            //self.pixmap.reset_clip();
            self.clips_count = former_clips_count;
            //self.clip_mask = clip_mask;
            self.transform = transform;
        }
    }

    /// Saves the entire state of the pixmap by pushing the current
    /// state onto a stack.
    pub fn save(&mut self) {
        self.saved_states.push(State {
            config: self.config.clone(),
            path_rect: self.path_rect,
            clips_count: self.clips_count,
            //clip_mask: self.clip_mask,
            transform: self.transform,
        });
    }

    /// Fill the background pixmap colors using their rgba8 values.
    pub fn start(&mut self) {
        self.pixmap.fill(tiny_skia::Color::from_rgba8(
            self.background.b(),
            self.background.g(),
            self.background.r(),
            self.background.a(),
        ));
    }

    /// Cleanup, once we are finished.
    pub fn finish(&mut self) {}
}
