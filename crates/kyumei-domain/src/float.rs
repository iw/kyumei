//! Floats as JSON can carry them.
//!
//! JSON has no NaN or infinity, and `serde_json` writes them as `null`, which
//! can't be read back as a number. Series from Prometheus are full of NaN (a
//! quantile over a step without traffic is 0/0), so a float that may not be
//! finite is written as Prometheus writes it: `"NaN"`, `"+Inf"` or `"-Inf"`.
//! Finite values stay numbers, and `null`, which `serde_json` writes for a float
//! that isn't finite, reads back as NaN.
//!
//! Use with `#[serde(with = "crate::float")]`.

use serde::{Deserialize, Deserializer, Serializer, de::Error};

pub fn serialize<S: Serializer>(value: &f64, s: S) -> Result<S::Ok, S::Error> {
    match *value {
        v if v.is_finite() => s.serialize_f64(v),
        v if v.is_nan() => s.serialize_str("NaN"),
        v if v > 0.0 => s.serialize_str("+Inf"),
        _ => s.serialize_str("-Inf"),
    }
}

pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<f64, D::Error> {
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum Repr {
        Number(f64),
        Text(String),
    }
    match Option::<Repr>::deserialize(d)? {
        None => Ok(f64::NAN),
        Some(Repr::Number(n)) => Ok(n),
        Some(Repr::Text(t)) => match t.as_str() {
            "NaN" | "nan" => Ok(f64::NAN),
            "+Inf" | "Inf" | "inf" | "+inf" => Ok(f64::INFINITY),
            "-Inf" | "-inf" => Ok(f64::NEG_INFINITY),
            other => other
                .parse()
                .map_err(|_| D::Error::custom(format!("`{other}` is not a number"))),
        },
    }
}

#[cfg(test)]
mod tests {
    use serde::{Deserialize, Serialize};

    #[derive(Debug, Serialize, Deserialize)]
    struct Sample {
        #[serde(with = "crate::float")]
        value: f64,
    }

    fn round_trip(v: f64) -> f64 {
        let json = serde_json::to_string(&Sample { value: v }).expect("write");
        let via_value: Sample =
            serde_json::from_value(serde_json::from_str(&json).expect("value")).expect("value");
        let direct: Sample = serde_json::from_str(&json).expect("read");
        assert_eq!(via_value.value.to_bits(), direct.value.to_bits());
        direct.value
    }

    #[test]
    fn non_finite_values_survive_json() {
        assert!(round_trip(f64::NAN).is_nan());
        assert_eq!(round_trip(f64::INFINITY), f64::INFINITY);
        assert_eq!(round_trip(f64::NEG_INFINITY), f64::NEG_INFINITY);
        assert_eq!(round_trip(0.0193), 0.0193);
        assert_eq!(
            serde_json::to_string(&Sample { value: 1.5 }).expect("write"),
            r#"{"value":1.5}"#,
            "finite values stay numbers"
        );
        assert_eq!(
            serde_json::to_string(&Sample { value: f64::NAN }).expect("write"),
            r#"{"value":"NaN"}"#
        );
        // As `serde_json` writes a float that isn't finite.
        let plain: Sample = serde_json::from_str(r#"{"value":null}"#).expect("null");
        assert!(plain.value.is_nan());
        assert!(serde_json::from_str::<Sample>(r#"{"value":"lots"}"#).is_err());
    }
}
