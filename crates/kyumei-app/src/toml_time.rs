//! Timestamps in TOML files may be TOML datetimes or strings.

use jiff::Timestamp;
use serde::{Deserialize, Deserializer, de::Error};

#[derive(Deserialize)]
#[serde(untagged)]
enum Repr {
    Toml(toml::value::Datetime),
    Text(String),
}

fn parse<E: Error>(repr: Repr) -> Result<Timestamp, E> {
    let text = match repr {
        Repr::Toml(dt) => dt.to_string(),
        Repr::Text(s) => s,
    };
    text.parse::<Timestamp>().map_err(|e| {
        E::custom(format!(
            "invalid timestamp `{text}` (use RFC 3339 with an offset): {e}"
        ))
    })
}

pub(crate) fn timestamp<'de, D: Deserializer<'de>>(d: D) -> Result<Timestamp, D::Error> {
    parse(Repr::deserialize(d)?)
}

pub(crate) fn option_timestamp<'de, D: Deserializer<'de>>(
    d: D,
) -> Result<Option<Timestamp>, D::Error> {
    Option::<Repr>::deserialize(d)?.map(parse).transpose()
}
