use anyhow::Context as _;
use gpui::{App, Menu, MenuItem, actions};
use objc2::{AnyThread, MainThreadMarker};
use objc2_app_kit::{NSApplication, NSImage};
use objc2_foundation::{NSData, NSUserDefaults, ns_string};
use release_channel::ReleaseChannel;
use util::ResultExt as _;

actions!(
    zed,
    [UseAmberDockIcon, UseMintDockIcon, UseOriginalDockIcon]
);

#[derive(Clone, Copy, PartialEq, Eq)]
#[repr(isize)]
pub(super) enum DockIcon {
    Amber = 0,
    Mint = 1,
    Original = 2,
}

impl DockIcon {
    const OPTIONS: [Self; 3] = [Self::Amber, Self::Mint, Self::Original];
    const LABELS: [&'static str; 3] = ["Amber–Coral–Magenta", "Mint–Blue", "Original"];

    pub(super) fn selected() -> Self {
        match NSUserDefaults::standardUserDefaults().integerForKey(ns_string!("ZedUnityDockIcon")) {
            1 => Self::Mint,
            2 => Self::Original,
            _ => Self::Amber,
        }
    }

    pub(super) fn bytes(self) -> &'static [u8] {
        match self {
            Self::Amber => include_bytes!("../../resources/app-icon-dev@2x.png"),
            Self::Mint => include_bytes!("../../resources/app-icon-unity-mint-blue.png"),
            Self::Original => include_bytes!("../../resources/app-icon@2x.png"),
        }
    }

    fn apply(self) -> anyhow::Result<()> {
        let main_thread =
            MainThreadMarker::new().context("setting the Dock icon off the main thread")?;
        let data = NSData::with_bytes(self.bytes());
        let image =
            NSImage::initWithData(NSImage::alloc(), &data).context("decoding the Dock icon")?;
        // AppKit retains the non-null image on the main thread.
        unsafe {
            NSApplication::sharedApplication(main_thread).setApplicationIconImage(Some(&image))
        };
        Ok(())
    }

    fn select(self, cx: &mut App) {
        if self.apply().log_err().is_some() {
            NSUserDefaults::standardUserDefaults()
                .setInteger_forKey(self as isize, ns_string!("ZedUnityDockIcon"));
            super::reload_menus(cx);
            cx.refresh_windows();
        }
    }
}

pub(super) fn init(cx: &mut App) {
    if ReleaseChannel::global(cx) == ReleaseChannel::Dev {
        DockIcon::selected().apply().log_err();
        cx.set_global(settings_ui::DockIconPicker {
            labels: &DockIcon::LABELS,
            selected: || DockIcon::selected() as usize,
            select: |index, cx| {
                if let Some(&icon) = DockIcon::OPTIONS.get(index) {
                    icon.select(cx);
                }
            },
        });
        cx.on_action(|_: &UseAmberDockIcon, cx| DockIcon::Amber.select(cx));
        cx.on_action(|_: &UseMintDockIcon, cx| DockIcon::Mint.select(cx));
        cx.on_action(|_: &UseOriginalDockIcon, cx| DockIcon::Original.select(cx));
    }
}

pub(super) fn menu_item() -> MenuItem {
    let selected = DockIcon::selected();
    MenuItem::submenu(
        Menu::new("Dock Icon").items([
            MenuItem::action("Amber–Coral–Magenta (Default)", UseAmberDockIcon)
                .checked(selected == DockIcon::Amber),
            MenuItem::action("Mint–Blue", UseMintDockIcon).checked(selected == DockIcon::Mint),
            MenuItem::action("Original", UseOriginalDockIcon)
                .checked(selected == DockIcon::Original),
        ]),
    )
}
