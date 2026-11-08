use std::path::PathBuf;
use std::sync::Arc;

use gpui_kit::{App, Menu, MenuItem, NoAction, OsAction, SystemMenuType, actions};
use workbook_scheduler::SystemClock;
use workbook_service::WorkbookService;

actions!(workbook_menu, [Quit]);

fn quit(_: &Quit, cx: &mut App) {
    cx.quit();
}

fn install_menu(cx: &mut App) {
    cx.on_action(quit);
    cx.set_menus([
        Menu::new("Workbook 2").items([
            MenuItem::os_submenu("Services", SystemMenuType::Services),
            MenuItem::separator(),
            MenuItem::action("Quit Workbook 2", Quit),
        ]),
        Menu::new("Edit").items([
            MenuItem::os_action("Undo", NoAction, OsAction::Undo),
            MenuItem::os_action("Redo", NoAction, OsAction::Redo),
            MenuItem::separator(),
            MenuItem::os_action("Cut", NoAction, OsAction::Cut),
            MenuItem::os_action("Copy", NoAction, OsAction::Copy),
            MenuItem::os_action("Paste", NoAction, OsAction::Paste),
            MenuItem::os_action("Select All", NoAction, OsAction::SelectAll),
        ]),
    ]);
}

fn data_directory() -> PathBuf {
    if let Ok(value) = std::env::var("WORKBOOK2_DATA_DIR") {
        return PathBuf::from(value);
    }
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
    PathBuf::from(home)
        .join("Library")
        .join("Application Support")
        .join("Workbook2")
}

fn main() -> anyhow::Result<()> {
    let data_dir = data_directory();
    std::fs::create_dir_all(&data_dir)?;
    let service = Arc::new(WorkbookService::open(&data_dir, Arc::new(SystemClock))?);
    if let Err(error) = service.daily_backup() {
        eprintln!("daily backup failed: {error}");
    }

    gpui_kit::application()
        .with_assets(gpui_kit::assets::Assets)
        .run(move |cx| {
            gpui_kit::init(cx);
            install_menu(cx);
            if let Err(error) = workbook_ui::open_window(service.clone(), cx) {
                eprintln!("failed to open Workbook 2 window: {error}");
                cx.quit();
                return;
            }
            cx.activate(true);
        });
    Ok(())
}
