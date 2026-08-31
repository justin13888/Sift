//! The Linux binary. D-61 puts it under Cargo, and the Flatpak manifest wraps it.

fn main() {
    #[cfg(all(target_os = "linux", feature = "toolkit"))]
    {
        sift_gtk::shell::run();
    }
    #[cfg(not(all(target_os = "linux", feature = "toolkit")))]
    {
        sift_gtk::shell::explain_why_this_is_not_the_binary_you_want();
    }
}
