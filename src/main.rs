#![windows_subsystem = "windows"]

mod app;
mod beat;
mod bluetooth;
mod cat;
// Later DM tasks (worker, UI) use these items; silence dead-code until then.
#[allow(dead_code)]
mod dm;
mod lyrics;
mod media;
mod peek;
mod pet;
mod pomodoro;
mod shelf;
mod status;
mod store;
mod sys;
mod todo;
mod ui;
mod water;

#[cfg(target_os = "linux")]
mod linux;
#[cfg(windows)]
mod win;

#[cfg(windows)]
fn main() -> windows::core::Result<()> {
    win::run()
}

#[cfg(target_os = "linux")]
fn main() {
    linux::run();
}
