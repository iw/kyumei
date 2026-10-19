//! Dates in pack files may be TOML dates (`2026-09-30`) or strings.

use jiff::civil::Date;
use serde::{Deserialize, Deserializer, de::Error};

#[derive(Deserialize)]
#[serde(untagged)]
enum DateRepr {
    Toml(toml::value::Datetime),
    Text(String),
}

fn parse<E: Error>(repr: DateRepr) -> Result<Date, E> {
    let text = match repr {
        DateRepr::Toml(dt) => match dt.date {
            Some(d) => format!("{:04}-{:02}-{:02}", d.year, d.month, d.day),
            None => return Err(E::custom("expected a date, found a time")),
        },
        DateRepr::Text(s) => s,
    };
    text.parse::<Date>()
        .map_err(|e| E::custom(format!("invalid date `{text}`: {e}")))
}

pub(crate) fn date<'de, D: Deserializer<'de>>(d: D) -> Result<Date, D::Error> {
    parse(DateRepr::deserialize(d)?)
}

pub(crate) fn option_date<'de, D: Deserializer<'de>>(d: D) -> Result<Option<Date>, D::Error> {
    Option::<DateRepr>::deserialize(d)?.map(parse).transpose()
}
