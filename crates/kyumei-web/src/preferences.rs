//! Per-browser preferences: the time zone pages are shown in.

use jiff::Timestamp;
use kyumei_domain::DisplayZone;
use serde::Deserialize;
use topcoat::{
    Result,
    context::Cx,
    router::{
        HeaderName, HeaderValue,
        content::Form,
        error::{SeeOther, see_other},
        header,
        request::headers,
        route,
    },
    view::{View, view},
};

use crate::pages::state;

/// The cookie that keeps a browser's time zone.
pub const ZONE_COOKIE: &str = "kyumei_tz";

/// The zone this request's pages are shown in: the browser's choice when the
/// configuration offers it, else UTC.
pub fn zone(cx: &Cx) -> DisplayZone {
    state(cx)
        .config
        .time_zones
        .resolve(cookie(cx, ZONE_COOKIE).as_deref())
}

fn cookie(cx: &Cx, name: &str) -> Option<String> {
    headers(cx)
        .get_all(header::COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|v| v.split(';'))
        .filter_map(|pair| pair.trim().split_once('='))
        .find(|(k, _)| *k == name)
        .map(|(_, v)| v.trim_matches('"').to_owned())
}

/// The time zone selector in the top bar.
#[topcoat::view::component]
pub async fn zone_selector(cx: &Cx) -> Result<impl View> {
    let st = state(cx).clone();
    let current = zone(cx);
    let now = Timestamp::now();
    let back = topcoat::router::request::uri(cx)
        .path_and_query()
        .map_or_else(|| "/".to_owned(), ToString::to_string);
    let groups: Vec<(String, Vec<(String, String)>)> = st
        .zone_choices()
        .iter()
        .map(|(region, zones)| {
            (
                region.clone(),
                zones
                    .iter()
                    .map(|z| (z.name().to_owned(), z.describe(now)))
                    .collect(),
            )
        })
        .collect();
    let current = current.name().to_owned();
    Ok(view! {
        <form method="post" action="/preferences/time-zone" class="zone-form">
            <input type="hidden" name="back" value=(back)>
            <label class="zone-label" title="Dates and times on every page are shown in this zone">
                <span class="zone-caption">"Times in"</span>
                <select name="zone" data-autosubmit="">
                    for (region, zones) in &groups {
                        if region.is_empty() {
                            for (name, text) in zones {
                                <option value=(name.clone()) selected=(*name == current)>(text.clone())</option>
                            }
                        } else {
                            <optgroup label=(region.clone())>
                                for (name, text) in zones {
                                    <option value=(name.clone()) selected=(*name == current)>(text.clone())</option>
                                }
                            </optgroup>
                        }
                    }
                </select>
            </label>
            <noscript><button type="submit" class="btn btn-quiet">"Set"</button></noscript>
        </form>
    })
}

#[derive(Debug, Deserialize)]
pub struct ZoneForm {
    zone: String,
    back: Option<String>,
}

/// Keep the chosen zone in a cookie, and go back to the page it was chosen on.
#[route(POST "/preferences/time-zone")]
pub async fn set_zone(
    cx: &Cx,
    Form(f): Form<ZoneForm>,
) -> Result<([(HeaderName, HeaderValue); 1], SeeOther)> {
    let chosen = state(cx).config.time_zones.resolve(Some(&f.zone));
    let cookie = format!(
        "{ZONE_COOKIE}={}; Path=/; Max-Age=31536000; SameSite=Lax; HttpOnly",
        chosen.name()
    );
    // Only a path on this site: never a redirect elsewhere.
    let back = f
        .back
        .filter(|b| b.starts_with('/') && !b.starts_with("//") && !b.contains('\\'))
        .unwrap_or_else(|| "/".to_owned());
    Ok((
        [(header::SET_COOKIE, HeaderValue::from_str(&cookie)?)],
        see_other(back),
    ))
}
