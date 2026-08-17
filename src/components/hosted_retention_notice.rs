use crate::Route;
use crate::backend::{HostedRetentionStatus, hosted_retention_status};
use dioxus::prelude::*;

#[component]
pub fn HostedRetentionNotice() -> Element {
    let retention_resource =
        use_server_future(move || async move { hosted_retention_status().await })?;
    let show_hosted_retention_notice = retention_resource()
        .and_then(|result| result.ok())
        .map(|status: HostedRetentionStatus| status.is_hosted && !status.active_paid)
        .unwrap_or(false);

    if show_hosted_retention_notice {
        rsx! {
            div {
                class: "alert alert-info",
                "data-testid": "hosted-retention-note",
                "On the hosted service, accounts are retained while you sign in regularly. Hosted accounts inactive for 180 days (6 months) are deleted. "
                " "
                Link { to: Route::WalletDataExport, title: "Go to data export page", "Export your data" }
                " to keep it available, then import it into a local or self-hosted BitGarth instance. "
                "Paid hosted data is retained, even without sign-in. "
                "We don't collect your email, so we can't warn you first."
            }
        }
    } else {
        rsx! {}
    }
}
