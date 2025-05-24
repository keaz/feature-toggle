use leptos::prelude::*;
use leptos::mount::mount_to_body;
use log::log;

fn main() {
    _ = console_log::init_with_level(log::Level::Debug);
    console_error_panic_hook::set_once();
    mount_to_body(|| view! { <App/> })
}

#[component]
fn App() -> impl IntoView {
    view! { <h1>"Feature Flag UI"</h1> }
}