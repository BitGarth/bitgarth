use dioxus::prelude::*;

/// Editorial section header (design-system §9.4): a copper `§ NN` numeral
/// next to a Fraunces headline. `emphasis`, when it is a substring of
/// `title`, is wrapped in a real `<em>` so screen readers get the emphasis.
#[component]
pub fn SectionHead(num: String, title: String, emphasis: Option<String>) -> Element {
    let parts = emphasis.as_deref().and_then(|em| {
        title
            .split_once(em)
            .map(|(before, after)| (before.to_string(), em.to_string(), after.to_string()))
    });

    rsx! {
        div { class: "section-head",
            span { class: "section-num", "§ {num}" }
            h2 { class: "section-title",
                if let Some((before, em, after)) = parts {
                    "{before}"
                    em { "{em}" }
                    "{after}"
                } else {
                    "{title}"
                }
            }
        }
    }
}
