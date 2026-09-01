//! The icon set.
//!
//! `DESIGN_SYSTEM.md` §5 specifies Lucide, rasterised from SVG into a texture
//! atlas at startup (L8). No SVG assets are vendored in this repository, so the
//! shapes are drawn directly with `epaint` on the same 24 × 24 grid, 1.5px
//! stroke, round caps and joins. That keeps every promise the design system
//! actually makes — one icon per concept, shape-distinct status marks, tinted
//! at draw time — without inventing an asset pipeline the repository cannot
//! feed.
//!
//! The concept → icon mapping in §5.4 is normative and lives in
//! [`Icon::for_destination_kind`] and friends, so one concept can never be two
//! icons in two places.

// The interface is a library-shaped tree inside a binary crate. Its components,
// view models and fixtures are also compiled by `crates/app/tests/gui_app.rs`
// as a separate crate, so items that are used and tested there look unused from
// the binary's side. The allow is scoped to this module rather than the crate.
#![allow(dead_code)]
use egui::{Color32, Painter, Pos2, Rect, Shape, Stroke, Vec2};

/// Every icon the interface uses, named after the Lucide glyph it draws.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Icon {
    // Navigation
    LayoutDashboard,
    Repeat,
    HardDrive,
    KeyRound,
    History,
    List,
    Settings,
    Info,
    // Domain
    Cloud,
    Database,
    FolderSync,
    Folder,
    FolderOpen,
    FileText,
    FilterX,
    Clock,
    Gauge,
    Archive,
    Terminal,
    GitBranch,
    Stethoscope,
    Sparkles,
    Shield,
    // Actions
    Play,
    Square,
    Pause,
    Lock,
    LockOpen,
    PlugZap,
    Copy,
    Eye,
    EyeOff,
    Trash,
    Pencil,
    Plus,
    ExternalLink,
    Download,
    Printer,
    Search,
    SearchX,
    // Status
    CheckCircle,
    AlertTriangle,
    XOctagon,
    RefreshCw,
    MinusCircle,
    Clock4,
    Circle,
    CircleDashed,
    // Chrome
    Check,
    X,
    ChevronDown,
    ChevronUp,
    ChevronRight,
    ChevronLeft,
    ArrowLeft,
    MoreHorizontal,
    MoreVertical,
}

impl Icon {
    /// The icon for a destination kind (`DESIGN_SYSTEM.md` §5.4).
    pub fn for_destination_kind(kind: &superbackup_core::model::DestinationKind) -> Icon {
        use superbackup_core::model::DestinationKind as K;
        match kind {
            K::LocalRepository { .. } => Icon::HardDrive,
            K::OneDrive { .. } => Icon::Cloud,
            K::S3 { .. } => Icon::Database,
            K::LocalMirror { .. } => Icon::FolderSync,
        }
    }

    /// The status icon, chosen so the five states stay distinct in greyscale.
    pub fn for_status(status: superbackup_core::state::RunStatus) -> Icon {
        use superbackup_core::state::RunStatus as R;
        match status {
            R::Succeeded => Icon::CheckCircle,
            R::SucceededWithWarnings => Icon::AlertTriangle,
            R::Failed => Icon::XOctagon,
            R::Running | R::Preparing | R::Finalising => Icon::RefreshCw,
            R::Queued => Icon::Clock4,
            R::Cancelled | R::Skipped => Icon::MinusCircle,
        }
    }

    pub fn for_health(health: superbackup_core::state::Health) -> Icon {
        use superbackup_core::state::Health as H;
        match health {
            H::Idle => Icon::CheckCircle,
            H::Running => Icon::RefreshCw,
            H::Attention => Icon::AlertTriangle,
            H::Paused => Icon::Pause,
            H::Failed => Icon::XOctagon,
        }
    }

    pub fn for_severity(severity: superbackup_core::state::Severity) -> Icon {
        use superbackup_core::state::Severity as S;
        match severity {
            S::Debug => Icon::CircleDashed,
            S::Info => Icon::Circle,
            S::Warning => Icon::AlertTriangle,
            S::Error => Icon::XOctagon,
        }
    }

    /// Draw into `rect`, tinted `color`. `rect` is expected to be square; the
    /// shape is centred and scaled from the 24-unit design grid.
    pub fn paint(self, painter: &Painter, rect: Rect, color: Color32) {
        self.paint_rotated(painter, rect, color, 0.0);
    }

    /// The same, rotated by `turns` (0..1) about the centre. Used only by the
    /// running mark, which turns at 0.75 revolutions per second.
    pub fn paint_rotated(self, painter: &Painter, rect: Rect, color: Color32, turns: f32) {
        let size = rect.width().min(rect.height());
        let scale = size / 24.0;
        let centre = rect.center();
        let (sin, cos) = (turns * std::f32::consts::TAU).sin_cos();

        let p = |x: f32, y: f32| -> Pos2 {
            let dx = (x - 12.0) * scale;
            let dy = (y - 12.0) * scale;
            if turns == 0.0 {
                centre + Vec2::new(dx, dy)
            } else {
                centre + Vec2::new(dx * cos - dy * sin, dx * sin + dy * cos)
            }
        };

        let width = (1.5 * scale).max(1.0);
        let stroke = Stroke::new(width, color);
        let mut d = Draw { painter, stroke, color, scale, p: &p };
        self.draw(&mut d);
    }
}

struct Draw<'a> {
    painter: &'a Painter,
    stroke: Stroke,
    color: Color32,
    scale: f32,
    p: &'a dyn Fn(f32, f32) -> Pos2,
}

impl Draw<'_> {
    fn pt(&self, x: f32, y: f32) -> Pos2 {
        (self.p)(x, y)
    }
    fn line(&self, x0: f32, y0: f32, x1: f32, y1: f32) {
        self.painter.line_segment([self.pt(x0, y0), self.pt(x1, y1)], self.stroke);
    }
    fn path(&self, pts: &[(f32, f32)]) {
        let points: Vec<Pos2> = pts.iter().map(|(x, y)| self.pt(*x, *y)).collect();
        self.painter.add(Shape::line(points, self.stroke));
    }
    fn closed(&self, pts: &[(f32, f32)]) {
        let points: Vec<Pos2> = pts.iter().map(|(x, y)| self.pt(*x, *y)).collect();
        self.painter.add(Shape::closed_line(points, self.stroke));
    }
    fn rect(&self, x: f32, y: f32, w: f32, h: f32, r: f32) {
        let min = self.pt(x, y);
        let max = self.pt(x + w, y + h);
        self.painter.rect_stroke(
            Rect::from_min_max(min, max),
            egui::CornerRadius::same((r * self.scale).round().clamp(0.0, 255.0) as u8),
            self.stroke,
            egui::StrokeKind::Middle,
        );
    }
    fn filled_rect(&self, x: f32, y: f32, w: f32, h: f32, r: f32) {
        let min = self.pt(x, y);
        let max = self.pt(x + w, y + h);
        self.painter.rect_filled(
            Rect::from_min_max(min, max),
            egui::CornerRadius::same((r * self.scale).round().clamp(0.0, 255.0) as u8),
            self.color,
        );
    }
    fn circle(&self, cx: f32, cy: f32, r: f32) {
        self.painter.circle_stroke(self.pt(cx, cy), r * self.scale, self.stroke);
    }
    fn dot(&self, cx: f32, cy: f32, r: f32) {
        self.painter.circle_filled(self.pt(cx, cy), r * self.scale, self.color);
    }
    /// Degrees, 0 = east, clockwise with y down.
    fn arc(&self, cx: f32, cy: f32, r: f32, from: f32, to: f32) {
        let steps = ((to - from).abs() / 12.0).ceil().max(3.0) as usize;
        let pts: Vec<Pos2> = (0..=steps)
            .map(|i| {
                let a = (from + (to - from) * i as f32 / steps as f32).to_radians();
                self.pt(cx + r * a.cos(), cy + r * a.sin())
            })
            .collect();
        self.painter.add(Shape::line(pts, self.stroke));
    }
    fn dashed_circle(&self, cx: f32, cy: f32, r: f32) {
        for i in 0..8 {
            let from = i as f32 * 45.0 + 6.0;
            self.arc(cx, cy, r, from, from + 33.0);
        }
    }
}

impl Icon {
    fn draw(self, d: &mut Draw<'_>) {
        match self {
            Icon::LayoutDashboard => {
                d.rect(3.0, 3.0, 7.0, 9.0, 1.5);
                d.rect(14.0, 3.0, 7.0, 5.0, 1.5);
                d.rect(14.0, 12.0, 7.0, 9.0, 1.5);
                d.rect(3.0, 16.0, 7.0, 5.0, 1.5);
            }
            Icon::Repeat => {
                d.path(&[(17.0, 2.0), (21.0, 6.0), (17.0, 10.0)]);
                d.path(&[(3.0, 11.0), (3.0, 9.0), (5.0, 6.0), (9.0, 6.0), (21.0, 6.0)]);
                d.path(&[(7.0, 22.0), (3.0, 18.0), (7.0, 14.0)]);
                d.path(&[(21.0, 13.0), (21.0, 15.0), (19.0, 18.0), (15.0, 18.0), (3.0, 18.0)]);
            }
            Icon::HardDrive => {
                d.line(22.0, 12.0, 2.0, 12.0);
                d.path(&[
                    (5.45, 5.11),
                    (2.0, 12.0),
                    (2.0, 18.0),
                    (22.0, 18.0),
                    (22.0, 12.0),
                    (18.55, 5.11),
                ]);
                d.line(6.0, 16.0, 6.01, 16.0);
                d.line(10.0, 16.0, 10.01, 16.0);
            }
            Icon::Cloud => {
                d.path(&[
                    (7.0, 19.0),
                    (4.5, 18.6),
                    (2.7, 16.6),
                    (2.6, 13.9),
                    (4.3, 11.7),
                    (6.8, 11.2),
                    (8.0, 8.2),
                    (11.0, 6.2),
                    (14.6, 6.6),
                    (17.2, 9.2),
                    (17.7, 11.6),
                    (20.5, 12.6),
                    (21.6, 15.4),
                    (20.3, 18.1),
                    (17.5, 19.0),
                    (7.0, 19.0),
                ]);
            }
            Icon::Database => {
                d.arc(12.0, 5.0, 9.0, 0.0, 180.0);
                d.arc(12.0, 5.0, 9.0, 180.0, 360.0);
                d.path(&[(3.0, 5.0), (3.0, 19.0)]);
                d.path(&[(21.0, 5.0), (21.0, 19.0)]);
                d.arc(12.0, 12.0, 9.0, 20.0, 160.0);
                d.arc(12.0, 19.0, 9.0, 20.0, 160.0);
            }
            Icon::FolderSync => {
                d.path(&[
                    (11.0, 20.0),
                    (4.0, 20.0),
                    (3.0, 19.0),
                    (3.0, 5.0),
                    (4.0, 4.0),
                    (9.0, 4.0),
                    (11.0, 6.0),
                    (20.0, 6.0),
                    (21.0, 7.0),
                    (21.0, 11.0),
                ]);
                d.path(&[(13.0, 15.5), (15.5, 13.0), (18.5, 13.0), (21.0, 15.5)]);
                d.path(&[(21.0, 12.5), (21.0, 15.5), (18.0, 15.5)]);
                d.path(&[(21.0, 19.0), (18.5, 21.5), (15.5, 21.5), (13.0, 19.0)]);
                d.path(&[(13.0, 22.0), (13.0, 19.0), (16.0, 19.0)]);
            }
            Icon::KeyRound => {
                d.circle(16.5, 7.5, 3.5);
                d.path(&[(14.0, 10.0), (2.5, 21.5)]);
                d.path(&[(5.0, 19.0), (7.0, 21.0)]);
                d.path(&[(7.5, 16.5), (9.5, 18.5)]);
            }
            Icon::History => {
                d.arc(12.0, 12.0, 9.0, 130.0, 400.0);
                d.path(&[(3.0, 5.0), (3.0, 11.0), (9.0, 11.0)]);
                d.path(&[(12.0, 7.0), (12.0, 12.0), (15.5, 14.0)]);
            }
            Icon::List => {
                d.line(8.0, 6.0, 21.0, 6.0);
                d.line(8.0, 12.0, 21.0, 12.0);
                d.line(8.0, 18.0, 21.0, 18.0);
                d.dot(3.5, 6.0, 1.1);
                d.dot(3.5, 12.0, 1.1);
                d.dot(3.5, 18.0, 1.1);
            }
            Icon::Settings => {
                d.circle(12.0, 12.0, 3.0);
                for i in 0..8 {
                    let a = (i as f32 * 45.0).to_radians();
                    let (sx, sy) = (12.0 + 6.0 * a.cos(), 12.0 + 6.0 * a.sin());
                    let (ex, ey) = (12.0 + 9.2 * a.cos(), 12.0 + 9.2 * a.sin());
                    d.line(sx, sy, ex, ey);
                }
                d.circle(12.0, 12.0, 6.0);
            }
            Icon::Info => {
                d.circle(12.0, 12.0, 9.5);
                d.line(12.0, 11.0, 12.0, 16.5);
                d.dot(12.0, 8.0, 1.05);
            }
            Icon::Folder => {
                d.closed(&[
                    (3.0, 19.5),
                    (3.0, 5.0),
                    (4.0, 4.0),
                    (9.0, 4.0),
                    (11.0, 6.5),
                    (20.0, 6.5),
                    (21.0, 7.5),
                    (21.0, 19.5),
                ]);
            }
            Icon::FolderOpen => {
                d.path(&[
                    (3.0, 19.5),
                    (3.0, 5.0),
                    (4.0, 4.0),
                    (9.0, 4.0),
                    (11.0, 6.5),
                    (19.0, 6.5),
                    (20.0, 7.5),
                    (20.0, 10.0),
                ]);
                d.path(&[(3.0, 19.5), (6.2, 11.0), (22.5, 11.0), (19.3, 19.5), (3.0, 19.5)]);
            }
            Icon::FileText => {
                d.path(&[
                    (14.0, 2.5),
                    (5.5, 2.5),
                    (4.5, 3.5),
                    (4.5, 20.5),
                    (5.5, 21.5),
                    (18.5, 21.5),
                    (19.5, 20.5),
                    (19.5, 8.0),
                    (14.0, 2.5),
                ]);
                d.path(&[(14.0, 2.5), (14.0, 8.0), (19.5, 8.0)]);
                d.line(8.0, 13.0, 16.0, 13.0);
                d.line(8.0, 17.0, 16.0, 17.0);
            }
            Icon::FilterX => {
                d.path(&[
                    (21.0, 3.0),
                    (3.0, 3.0),
                    (10.0, 11.5),
                    (10.0, 20.0),
                    (14.0, 18.0),
                    (14.0, 11.5),
                    (17.0, 8.0),
                ]);
                d.line(18.0, 12.0, 22.5, 16.5);
                d.line(22.5, 12.0, 18.0, 16.5);
            }
            Icon::Clock | Icon::Clock4 => {
                d.circle(12.0, 12.0, 9.5);
                if self == Icon::Clock4 {
                    d.path(&[(12.0, 6.5), (12.0, 12.0), (16.0, 14.5)]);
                } else {
                    d.path(&[(12.0, 6.5), (12.0, 12.0), (16.5, 12.0)]);
                }
            }
            Icon::Gauge => {
                d.arc(12.0, 14.0, 9.0, 180.0, 360.0);
                d.line(12.0, 14.0, 17.0, 9.0);
                d.line(3.0, 14.0, 21.0, 14.0);
            }
            Icon::Archive => {
                d.rect(2.5, 3.5, 19.0, 4.5, 1.5);
                d.path(&[(4.5, 8.0), (4.5, 20.0), (19.5, 20.0), (19.5, 8.0)]);
                d.line(9.5, 12.0, 14.5, 12.0);
            }
            Icon::Terminal => {
                d.path(&[(5.0, 7.0), (10.0, 12.0), (5.0, 17.0)]);
                d.line(12.0, 18.0, 19.0, 18.0);
            }
            Icon::GitBranch => {
                d.line(6.0, 4.0, 6.0, 14.0);
                d.circle(6.0, 17.0, 2.6);
                d.circle(18.0, 7.0, 2.6);
                d.path(&[(18.0, 9.6), (18.0, 12.0), (15.0, 15.0), (8.6, 15.5)]);
            }
            Icon::Stethoscope => {
                d.path(&[
                    (4.5, 3.0),
                    (4.5, 8.0),
                    (7.5, 12.0),
                    (11.5, 12.0),
                    (14.5, 8.0),
                    (14.5, 3.0),
                ]);
                d.path(&[(9.5, 12.0), (9.5, 15.5), (13.5, 18.5), (17.5, 16.5)]);
                d.circle(19.0, 14.0, 2.6);
            }
            Icon::Sparkles => {
                d.closed(&[
                    (9.0, 3.0),
                    (10.6, 7.4),
                    (15.0, 9.0),
                    (10.6, 10.6),
                    (9.0, 15.0),
                    (7.4, 10.6),
                    (3.0, 9.0),
                    (7.4, 7.4),
                ]);
                d.closed(&[
                    (18.0, 13.0),
                    (18.9, 15.1),
                    (21.0, 16.0),
                    (18.9, 16.9),
                    (18.0, 19.0),
                    (17.1, 16.9),
                    (15.0, 16.0),
                    (17.1, 15.1),
                ]);
            }
            Icon::Shield => {
                d.closed(&[
                    (12.0, 2.5),
                    (20.0, 5.5),
                    (20.0, 11.5),
                    (12.0, 21.5),
                    (4.0, 11.5),
                    (4.0, 5.5),
                ]);
            }
            Icon::Play => {
                let pts = [(7.0, 4.5), (19.5, 12.0), (7.0, 19.5)];
                let points: Vec<Pos2> = pts.iter().map(|(x, y)| d.pt(*x, *y)).collect();
                d.painter.add(Shape::convex_polygon(points, d.color, Stroke::NONE));
            }
            Icon::Square => {
                d.filled_rect(6.0, 6.0, 12.0, 12.0, 1.5);
            }
            Icon::Pause => {
                d.filled_rect(7.0, 4.5, 3.6, 15.0, 1.4);
                d.filled_rect(13.4, 4.5, 3.6, 15.0, 1.4);
            }
            Icon::Lock => {
                d.rect(4.0, 10.5, 16.0, 10.0, 2.0);
                d.arc(12.0, 10.5, 4.5, 180.0, 360.0);
                d.dot(12.0, 15.5, 1.2);
            }
            Icon::LockOpen => {
                d.rect(4.0, 10.5, 16.0, 10.0, 2.0);
                d.arc(15.5, 10.5, 4.5, 180.0, 300.0);
                d.dot(12.0, 15.5, 1.2);
            }
            Icon::PlugZap => {
                d.path(&[(7.0, 2.0), (7.0, 6.0)]);
                d.path(&[(15.0, 2.0), (15.0, 6.0)]);
                d.path(&[
                    (4.5, 6.0),
                    (17.5, 6.0),
                    (17.5, 10.0),
                    (15.0, 13.5),
                    (7.0, 13.5),
                    (4.5, 10.0),
                    (4.5, 6.0),
                ]);
                d.path(&[(12.5, 13.5), (9.5, 18.0), (13.5, 18.0), (10.5, 22.0)]);
            }
            Icon::Copy => {
                d.rect(8.5, 8.5, 12.0, 12.0, 2.0);
                d.path(&[(5.5, 15.5), (3.5, 15.5), (3.5, 3.5), (15.5, 3.5), (15.5, 5.5)]);
            }
            Icon::Eye => {
                d.path(&[(2.0, 12.0), (5.5, 7.0), (12.0, 5.0), (18.5, 7.0), (22.0, 12.0)]);
                d.path(&[(2.0, 12.0), (5.5, 17.0), (12.0, 19.0), (18.5, 17.0), (22.0, 12.0)]);
                d.circle(12.0, 12.0, 3.0);
            }
            Icon::EyeOff => {
                d.path(&[(2.5, 12.5), (6.0, 8.0), (10.0, 6.2)]);
                d.path(&[(14.5, 6.6), (19.0, 9.0), (21.5, 12.5), (18.0, 16.5)]);
                d.path(&[(14.0, 18.4), (10.0, 18.6), (5.5, 16.5), (2.5, 12.5)]);
                d.line(3.0, 3.0, 21.0, 21.0);
            }
            Icon::Trash => {
                d.line(3.5, 6.0, 20.5, 6.0);
                d.path(&[(8.0, 6.0), (8.0, 3.5), (16.0, 3.5), (16.0, 6.0)]);
                d.path(&[(5.5, 6.0), (6.5, 20.5), (17.5, 20.5), (18.5, 6.0)]);
                d.line(10.0, 10.0, 10.0, 17.0);
                d.line(14.0, 10.0, 14.0, 17.0);
            }
            Icon::Pencil => {
                d.path(&[
                    (16.5, 3.5),
                    (20.5, 7.5),
                    (8.0, 20.0),
                    (3.0, 21.0),
                    (4.0, 16.0),
                    (16.5, 3.5),
                ]);
                d.line(14.5, 5.5, 18.5, 9.5);
            }
            Icon::Plus => {
                d.line(12.0, 5.0, 12.0, 19.0);
                d.line(5.0, 12.0, 19.0, 12.0);
            }
            Icon::ExternalLink => {
                d.path(&[(14.0, 4.0), (20.0, 4.0), (20.0, 10.0)]);
                d.line(10.0, 14.0, 20.0, 4.0);
                d.path(&[(17.0, 13.0), (17.0, 19.0), (5.0, 19.0), (5.0, 7.0), (11.0, 7.0)]);
            }
            Icon::Download => {
                d.line(12.0, 3.5, 12.0, 15.5);
                d.path(&[(7.0, 10.5), (12.0, 15.5), (17.0, 10.5)]);
                d.path(&[(4.0, 19.5), (20.0, 19.5)]);
            }
            Icon::Printer => {
                d.path(&[(7.0, 8.0), (7.0, 3.0), (17.0, 3.0), (17.0, 8.0)]);
                d.path(&[
                    (7.0, 17.5),
                    (4.0, 17.5),
                    (3.0, 16.5),
                    (3.0, 10.0),
                    (4.0, 9.0),
                    (20.0, 9.0),
                    (21.0, 10.0),
                    (21.0, 16.5),
                    (20.0, 17.5),
                    (17.0, 17.5),
                ]);
                d.rect(7.0, 14.0, 10.0, 7.0, 1.0);
            }
            Icon::Search | Icon::SearchX => {
                d.circle(10.5, 10.5, 6.5);
                d.line(15.5, 15.5, 20.5, 20.5);
                if self == Icon::SearchX {
                    d.line(8.3, 8.3, 12.7, 12.7);
                    d.line(12.7, 8.3, 8.3, 12.7);
                }
            }
            Icon::CheckCircle => {
                d.circle(12.0, 12.0, 9.5);
                d.path(&[(7.8, 12.2), (10.8, 15.2), (16.4, 9.0)]);
            }
            Icon::AlertTriangle => {
                d.closed(&[(12.0, 3.0), (22.0, 20.5), (2.0, 20.5)]);
                d.line(12.0, 9.5, 12.0, 14.5);
                d.dot(12.0, 17.6, 1.05);
            }
            Icon::XOctagon => {
                d.closed(&[
                    (8.2, 2.5),
                    (15.8, 2.5),
                    (21.5, 8.2),
                    (21.5, 15.8),
                    (15.8, 21.5),
                    (8.2, 21.5),
                    (2.5, 15.8),
                    (2.5, 8.2),
                ]);
                d.line(8.6, 8.6, 15.4, 15.4);
                d.line(15.4, 8.6, 8.6, 15.4);
            }
            Icon::RefreshCw => {
                d.arc(12.0, 12.0, 8.0, -60.0, 150.0);
                d.path(&[(20.0, 4.0), (20.0, 9.4), (14.6, 9.4)]);
                d.arc(12.0, 12.0, 8.0, 120.0, 330.0);
                d.path(&[(4.0, 20.0), (4.0, 14.6), (9.4, 14.6)]);
            }
            Icon::MinusCircle => {
                d.circle(12.0, 12.0, 9.5);
                d.line(7.8, 12.0, 16.2, 12.0);
            }
            Icon::Circle => {
                d.circle(12.0, 12.0, 9.5);
            }
            Icon::CircleDashed => {
                d.dashed_circle(12.0, 12.0, 9.5);
            }
            Icon::Check => {
                d.path(&[(5.0, 12.5), (9.8, 17.3), (19.0, 6.8)]);
            }
            Icon::X => {
                d.line(5.5, 5.5, 18.5, 18.5);
                d.line(18.5, 5.5, 5.5, 18.5);
            }
            Icon::ChevronDown => d.path(&[(6.0, 9.5), (12.0, 15.5), (18.0, 9.5)]),
            Icon::ChevronUp => d.path(&[(6.0, 14.5), (12.0, 8.5), (18.0, 14.5)]),
            Icon::ChevronRight => d.path(&[(9.5, 6.0), (15.5, 12.0), (9.5, 18.0)]),
            Icon::ChevronLeft => d.path(&[(14.5, 6.0), (8.5, 12.0), (14.5, 18.0)]),
            Icon::ArrowLeft => {
                d.line(19.0, 12.0, 5.0, 12.0);
                d.path(&[(11.0, 6.0), (5.0, 12.0), (11.0, 18.0)]);
            }
            Icon::MoreHorizontal => {
                d.dot(5.5, 12.0, 1.3);
                d.dot(12.0, 12.0, 1.3);
                d.dot(18.5, 12.0, 1.3);
            }
            Icon::MoreVertical => {
                d.dot(12.0, 5.5, 1.3);
                d.dot(12.0, 12.0, 1.3);
                d.dot(12.0, 18.5, 1.3);
            }
        }
    }
}

/// The health mark — the tray icon, drawn live at any size
/// (`DESIGN_SYSTEM.md` §7). Shape carries the state in all five cases so the
/// mark survives greyscale and the macOS template treatment.
///
/// `spin` is the running animation phase in turns.
pub fn health_mark(
    painter: &Painter,
    rect: Rect,
    health: superbackup_core::state::Health,
    ink: Color32,
    pip: Option<(Color32, Color32)>,
    spin: f32,
) {
    use superbackup_core::state::Health as H;

    let size = rect.width().min(rect.height());
    let c = rect.center();

    // The same five badge silhouettes the tray draws.
    //
    // This used to be the retired ring-and-pip, so the window showed one mark
    // and the notification area a different one — for the same five states.
    // Shape carries the state here too: a filled disc, an open ring, a
    // triangle, two bars, a cross. None is distinguished by colour alone.
    let badge = pip.map(|(fill, _)| fill).unwrap_or(ink);
    let br = size * 0.36;
    let stroke_w = (br * 0.42).max(1.5);

    match health {
        H::Idle => {
            painter.circle_filled(c, br, badge);
        }
        H::Running => {
            // The only badge with a hole, and the only one that moves.
            let gap = 70.0_f32;
            let from = spin;
            let sweep = 360.0 - gap;
            let steps = 40;
            let rr = br - stroke_w / 2.0;
            let pts: Vec<Pos2> = (0..=steps)
                .map(|i| {
                    let a = (from + sweep * i as f32 / steps as f32).to_radians();
                    c + Vec2::new(rr * a.cos(), rr * a.sin())
                })
                .collect();
            painter.add(Shape::line(pts, Stroke::new(stroke_w, badge)));
        }
        H::Attention => {
            // The only badge with a flat base and a point.
            let t = [
                c + Vec2::new(0.0, -br),
                c + Vec2::new(br * 0.92, br * 0.72),
                c + Vec2::new(-br * 0.92, br * 0.72),
            ];
            painter.add(Shape::convex_polygon(t.to_vec(), badge, Stroke::NONE));
        }
        H::Paused => {
            // The only badge that is not a single shape.
            let w = br * 0.34;
            let h = br * 1.3;
            for dx in [-br * 0.42, br * 0.42] {
                painter.rect_filled(
                    Rect::from_center_size(c + Vec2::new(dx, 0.0), Vec2::new(w, h)),
                    w * 0.5,
                    badge,
                );
            }
        }
        H::Failed => {
            // The only badge with diagonals. A diagonal spreads across two
            // pixel columns, so it carries a slightly heavier stroke to weigh
            // the same as the others.
            let d = br * 0.62;
            let w = Stroke::new(stroke_w * 1.2, badge);
            painter.line_segment([c + Vec2::new(-d, -d), c + Vec2::new(d, d)], w);
            painter.line_segment([c + Vec2::new(d, -d), c + Vec2::new(-d, d)], w);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use superbackup_core::model::DestinationKind;
    use superbackup_core::state::RunStatus;

    #[test]
    fn destination_kinds_map_to_the_documented_icons() {
        assert_eq!(
            Icon::for_destination_kind(&DestinationKind::LocalRepository {
                path: PathBuf::from("/tmp")
            }),
            Icon::HardDrive
        );
        assert_eq!(
            Icon::for_destination_kind(&DestinationKind::LocalMirror {
                path: PathBuf::from("/tmp")
            }),
            Icon::FolderSync
        );
    }

    #[test]
    fn success_and_failure_have_different_silhouettes() {
        // The whole point of the octagon: greyscale still distinguishes them.
        assert_ne!(Icon::for_status(RunStatus::Succeeded), Icon::for_status(RunStatus::Failed));
        assert_eq!(Icon::for_status(RunStatus::Failed), Icon::XOctagon);
        assert_eq!(Icon::for_status(RunStatus::SucceededWithWarnings), Icon::AlertTriangle);
    }

    #[test]
    fn every_active_status_uses_the_same_running_mark() {
        for s in [RunStatus::Running, RunStatus::Preparing, RunStatus::Finalising] {
            assert_eq!(Icon::for_status(s), Icon::RefreshCw);
        }
    }
}
