//! Where the window is, and how it got there.
//!
//! Eight rail items — seven destinations plus About — in the fixed order of
//! `UX_SPEC.md` §1, and the sub-screens that push on top of them. Projects are
//! deliberately not a rail item: a project groups jobs, it does not own
//! anything, and making it a destination would imply that it does.

// The interface is a library-shaped tree inside a binary crate. Its components,
// view models and fixtures are also compiled by `crates/app/tests/gui_app.rs`
// as a separate crate, so items that are used and tested there look unused from
// the binary's side. The allow is scoped to this module rather than the crate.
#![allow(dead_code)]
use uuid::Uuid;

use super::copy;
use super::icons::Icon;

/// The eight rail items.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Section {
    Dashboard,
    Jobs,
    Destinations,
    Providers,
    Restore,
    Activity,
    Settings,
    About,
}

impl Section {
    pub const ALL: [Section; 8] = [
        Section::Dashboard,
        Section::Jobs,
        Section::Destinations,
        Section::Providers,
        Section::Restore,
        Section::Activity,
        Section::Settings,
        Section::About,
    ];

    pub fn title(self) -> &'static str {
        match self {
            Section::Dashboard => "Dashboard",
            Section::Jobs => copy::jobs::TITLE,
            Section::Destinations => copy::dest::TITLE,
            Section::Providers => copy::prov::TITLE,
            Section::Restore => copy::restore::TITLE,
            Section::Activity => copy::activity::TITLE,
            Section::Settings => copy::settings::TITLE,
            Section::About => "About",
        }
    }

    /// The collapsed rail shows only this, so the label lives in the tooltip.
    pub fn icon(self) -> Icon {
        match self {
            Section::Dashboard => Icon::LayoutDashboard,
            Section::Jobs => Icon::Repeat,
            Section::Destinations => Icon::HardDrive,
            Section::Providers => Icon::KeyRound,
            Section::Restore => Icon::History,
            Section::Activity => Icon::List,
            Section::Settings => Icon::Settings,
            Section::About => Icon::Info,
        }
    }

    /// `Ctrl/Cmd + 1…7`. About has no shortcut, matching the spec's table.
    pub fn shortcut(self) -> Option<egui::Key> {
        match self {
            Section::Dashboard => Some(egui::Key::Num1),
            Section::Jobs => Some(egui::Key::Num2),
            Section::Destinations => Some(egui::Key::Num3),
            Section::Providers => Some(egui::Key::Num4),
            Section::Restore => Some(egui::Key::Num5),
            Section::Activity => Some(egui::Key::Num6),
            Section::Settings => Some(egui::Key::Num7),
            Section::About => None,
        }
    }

    /// A 12px gap sits above Settings, separating navigation from preferences.
    pub fn gap_before(self) -> bool {
        self == Section::Settings
    }

    pub fn route(self) -> Route {
        match self {
            Section::Dashboard => Route::Dashboard,
            Section::Jobs => Route::Jobs,
            Section::Destinations => Route::Destinations,
            Section::Providers => Route::Providers,
            Section::Restore => Route::Restore,
            Section::Activity => Route::Activity,
            Section::Settings => Route::Settings(SettingsSection::General),
            Section::About => Route::About,
        }
    }
}

/// The nine settings sections (`UX_SPEC.md` §12).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SettingsSection {
    General,
    Scheduling,
    Bandwidth,
    Notifications,
    Security,
    Kopia,
    Remote,
    Advanced,
    Reset,
}

impl SettingsSection {
    pub const ALL: [SettingsSection; 9] = [
        SettingsSection::General,
        SettingsSection::Scheduling,
        SettingsSection::Bandwidth,
        SettingsSection::Notifications,
        SettingsSection::Security,
        SettingsSection::Kopia,
        SettingsSection::Remote,
        SettingsSection::Advanced,
        SettingsSection::Reset,
    ];
    pub fn title(self) -> &'static str {
        use copy::settings as s;
        match self {
            SettingsSection::General => s::SECTION_GENERAL,
            SettingsSection::Scheduling => s::SECTION_SCHEDULING,
            SettingsSection::Bandwidth => s::SECTION_BANDWIDTH,
            SettingsSection::Notifications => s::SECTION_NOTIFICATIONS,
            SettingsSection::Security => s::SECTION_SECURITY,
            SettingsSection::Kopia => s::SECTION_KOPIA,
            SettingsSection::Remote => s::SECTION_REMOTE,
            SettingsSection::Advanced => s::SECTION_ADVANCED,
            SettingsSection::Reset => s::SECTION_RESET,
        }
    }
}

/// A screen, including the sub-screens that push over a section.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Route {
    Dashboard,
    Jobs,
    /// The five-tab editor. `None` is a job that no longer exists, which the
    /// editor renders as an error state rather than a blank screen.
    JobEditor(Uuid),
    Destinations,
    DestinationEditor(Uuid),
    /// A destination being created, not yet saved.
    NewDestination,
    Providers,
    ProviderEditor(Uuid),
    NewProvider,
    Restore,
    Activity,
    RunDetail(Uuid),
    Settings(SettingsSection),
    About,
}

impl Route {
    /// The rail item this route belongs to, so a pushed sub-screen keeps its
    /// section marked as current.
    pub fn section(&self) -> Section {
        match self {
            Route::Dashboard => Section::Dashboard,
            Route::Jobs | Route::JobEditor(_) => Section::Jobs,
            Route::Destinations | Route::DestinationEditor(_) | Route::NewDestination => {
                Section::Destinations
            }
            Route::Providers | Route::ProviderEditor(_) | Route::NewProvider => Section::Providers,
            Route::Restore => Section::Restore,
            Route::Activity | Route::RunDetail(_) => Section::Activity,
            Route::Settings(_) => Section::Settings,
            Route::About => Section::About,
        }
    }

    /// True for the pushed screens, which show a breadcrumb and a back button.
    pub fn is_sub_screen(&self) -> bool {
        matches!(
            self,
            Route::JobEditor(_)
                | Route::DestinationEditor(_)
                | Route::NewDestination
                | Route::ProviderEditor(_)
                | Route::NewProvider
                | Route::RunDetail(_)
        )
    }

    /// Where the back button goes.
    pub fn parent(&self) -> Route {
        self.section().route()
    }

    /// Every route the interface can be in, for the render smoke tests. The
    /// editor routes appear twice: once against the fixture ids, and once
    /// against an id that does not exist, because a job deleted in another
    /// window must not take the interface down.
    pub fn every() -> Vec<Route> {
        let mut routes = vec![
            Route::Dashboard,
            Route::Jobs,
            Route::JobEditor(super::fixtures::JOB_DEV),
            Route::JobEditor(Uuid::nil()),
            Route::Destinations,
            Route::DestinationEditor(super::fixtures::DEST_LOCAL),
            Route::DestinationEditor(super::fixtures::DEST_S3),
            Route::DestinationEditor(super::fixtures::DEST_MIRROR),
            Route::DestinationEditor(super::fixtures::DEST_ONEDRIVE),
            Route::DestinationEditor(Uuid::nil()),
            Route::NewDestination,
            Route::Providers,
            Route::ProviderEditor(super::fixtures::PROVIDER_STORJ),
            Route::ProviderEditor(Uuid::nil()),
            Route::NewProvider,
            Route::Restore,
            Route::Activity,
            Route::RunDetail(super::fixtures::RUN_PARTIAL),
            Route::RunDetail(Uuid::nil()),
            Route::About,
        ];
        routes.extend(SettingsSection::ALL.iter().map(|s| Route::Settings(*s)));
        routes
    }
}

/// A small back-stack, so `Escape` and the breadcrumb go where the user came
/// from rather than always to the section root.
#[derive(Debug, Clone)]
pub struct Nav {
    current: Route,
    history: Vec<Route>,
}

impl Nav {
    pub fn new() -> Nav {
        Nav { current: Route::Dashboard, history: Vec::new() }
    }

    pub fn current(&self) -> &Route {
        &self.current
    }

    pub fn go(&mut self, route: Route) {
        if route == self.current {
            return;
        }
        self.history.push(self.current.clone());
        if self.history.len() > 32 {
            self.history.remove(0);
        }
        self.current = route;
    }

    /// Back to the previous screen, or to this one's parent when there is no
    /// history — never to nothing.
    pub fn back(&mut self) {
        match self.history.pop() {
            Some(previous) => self.current = previous,
            None => self.current = self.current.parent(),
        }
    }

    pub fn can_go_back(&self) -> bool {
        !self.history.is_empty() || self.current.is_sub_screen()
    }
}

impl Default for Nav {
    fn default() -> Self {
        Nav::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_rail_is_eight_items_in_the_documented_order() {
        assert_eq!(Section::ALL.len(), 8);
        assert_eq!(Section::ALL[0], Section::Dashboard);
        assert_eq!(Section::ALL[6], Section::Settings);
        assert_eq!(Section::ALL[7], Section::About);
        assert!(Section::Settings.gap_before());
        assert!(Section::About.shortcut().is_none());
    }

    #[test]
    fn sub_screens_keep_their_section_selected() {
        assert_eq!(Route::JobEditor(Uuid::nil()).section(), Section::Jobs);
        assert_eq!(Route::RunDetail(Uuid::nil()).section(), Section::Activity);
        assert_eq!(Route::NewProvider.section(), Section::Providers);
    }

    #[test]
    fn back_falls_through_to_the_parent_rather_than_to_nothing() {
        let mut nav = Nav::new();
        nav.go(Route::RunDetail(Uuid::nil()));
        nav.back();
        assert_eq!(*nav.current(), Route::Dashboard);

        let mut nav = Nav::new();
        nav.current = Route::JobEditor(Uuid::nil());
        nav.back();
        assert_eq!(*nav.current(), Route::Jobs);
    }

    #[test]
    fn navigating_to_the_current_route_does_not_grow_the_history() {
        let mut nav = Nav::new();
        nav.go(Route::Dashboard);
        assert!(!nav.can_go_back());
    }

    #[test]
    fn every_settings_section_is_reachable() {
        let routes = Route::every();
        for section in SettingsSection::ALL {
            assert!(routes.contains(&Route::Settings(section)), "{section:?}");
        }
    }
}
