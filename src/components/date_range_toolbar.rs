use dioxus::prelude::*;

use super::icons::{ChevronLeftIcon, ChevronRightIcon};

#[component]
pub(crate) fn DateRangeToolbar(
    start_input_id: String,
    start_input_value: String,
    end_input_id: String,
    end_input_value: String,
    validation_message: Option<String>,
    disable_date_inputs: bool,
    year_label: String,
    disable_previous_year: bool,
    disable_next_year: bool,
    show_this_year: bool,
    custom_range_open: bool,
    on_previous_year: EventHandler<()>,
    on_next_year: EventHandler<()>,
    on_this_year: EventHandler<()>,
    on_start_change: EventHandler<String>,
    on_end_change: EventHandler<String>,
    #[props(default)] right_side_extension: Option<Element>,
    #[props(default)] secondary_row: Option<Element>,
) -> Element {
    let mut custom_open = use_signal(|| custom_range_open);
    let custom_is_open = custom_open();

    rsx! {
        div { class: "date-range-toolbar",
            div { class: "date-range-toolbar-main",
                div { class: "date-range-year-dial",
                    button {
                        class: "date-range-year-step",
                        r#type: "button",
                        "aria-label": "Previous year",
                        disabled: disable_previous_year,
                        onclick: move |_| on_previous_year.call(()),
                        ChevronLeftIcon {}
                    }
                    span { class: "date-range-year-value", "{year_label}" }
                    button {
                        class: "date-range-year-step",
                        r#type: "button",
                        "aria-label": "Next year",
                        disabled: disable_next_year,
                        onclick: move |_| on_next_year.call(()),
                        ChevronRightIcon {}
                    }
                }

                if show_this_year {
                    button {
                        class: "btn btn-outline date-range-this-year",
                        r#type: "button",
                        onclick: move |_| on_this_year.call(()),
                        "This Year"
                    }
                }

                button {
                    class: "date-range-custom-toggle",
                    r#type: "button",
                    "aria-expanded": "{custom_is_open}",
                    onclick: move |_| custom_open.set(!custom_open()),
                    if custom_is_open { "Hide custom dates" } else { "Custom range…" }
                }

                div { class: "date-range-toolbar-right",
                    if let Some(extension) = right_side_extension {
                        div { class: "date-range-toolbar-right-extension",
                            {extension}
                        }
                    }
                }
            }

            if custom_is_open {
                div { class: "date-range-custom-fields",
                    div { class: "date-range-toolbar-date-group",
                        label { class: "form-label", r#for: "{start_input_id}", "Start Date" }
                        input {
                            id: "{start_input_id}",
                            class: "form-input",
                            r#type: "date",
                            disabled: disable_date_inputs,
                            value: "{start_input_value}",
                            onchange: move |event| on_start_change.call(event.value()),
                        }
                    }

                    div { class: "date-range-toolbar-date-group",
                        label { class: "form-label", r#for: "{end_input_id}", "End Date" }
                        input {
                            id: "{end_input_id}",
                            class: "form-input",
                            r#type: "date",
                            disabled: disable_date_inputs,
                            value: "{end_input_value}",
                            onchange: move |event| on_end_change.call(event.value()),
                        }
                    }
                }
            }

            if let Some(message) = validation_message {
                p { class: "date-range-toolbar-validation", "{message}" }
            }

            if let Some(row) = secondary_row {
                div { class: "date-range-toolbar-secondary",
                    {row}
                }
            }
        }
    }
}
