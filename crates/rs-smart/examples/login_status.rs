//! Prints login-item status. Run from inside a .app to exercise SMAppService
//! (read-only: never registers).
fn main() {
    println!("bundled={} status={:?}", rs_smart::login_item::is_running_from_app_bundle(), rs_smart::login_item::login_item_status());
    if let Err(e) = rs_smart::login_item::set_launch_at_login(false) {
        println!("set(false) -> {e}");
    } else {
        println!("set(false) -> ok");
    }
}
