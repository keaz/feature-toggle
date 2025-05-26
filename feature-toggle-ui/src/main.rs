use std::cmp::max;
use std::marker::PhantomData;
use leptos::prelude::*;
use leptos::mount::mount_to_body;
use log::log;

fn main() {
    _ = console_log::init_with_level(log::Level::Debug);
    console_error_panic_hook::set_once();
    mount_to_body(App)
}

#[component]
fn SizeOf<T: Sized>(#[prop(optional)] _ty: PhantomData<T>) -> impl IntoView {
    size_of::<T>()
}

#[component]
fn App() -> impl IntoView {
    let length = 5;
    let counters = (1..=length).map(|idx| RwSignal::new(idx));
    let values = vec![10,11,0,8];
    let counter_buttons = counters.map(|counter| {
        view! {
            <li>
                <button on:click=move |_| *counter.write() += 1 >
                {counter}
                </button>
            </li>
        }
    }).collect_view();

    view! {
        
        <ul>
            {counter_buttons}
        </ul>
    }
}

/// Show progress of the signal
#[component]
fn ProgressBar(
    /// The maximum value of the progress bar
    #[prop(default=100)]
    max: u16,
    #[prop(into)]
    progress: Signal<i32> ) -> impl IntoView {
    view! {
        <progress
            max=max
            value=progress
        />
        <br/>
    }
}