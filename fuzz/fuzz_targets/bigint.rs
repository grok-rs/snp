//! `BigInt` parsing and formatting never panic, and round-trip.
#![no_main]

use cfgv::{BigInt, ParseBigIntError, Value};
use libfuzzer_sys::fuzz_target;

fuzz_target!(|input: (&str, u8)| {
    let (text, radix) = input;
    let radix = u32::from(radix % 40);
    match BigInt::from_str_radix(text, radix) {
        Ok(big) => {
            let hex = format!("{big:x}");
            assert_eq!(BigInt::from_str_radix(&hex, 16).ok(), Some(big.clone()));
            if let Ok(decimal) = big.to_decimal_string() {
                assert!(!big.exceeds_max_str_digits());
                assert_eq!(decimal, big.to_string());
                assert_eq!(decimal.parse::<BigInt>().ok(), Some(big.clone()));
            } else {
                assert!(big.exceeds_max_str_digits());
                assert!(Value::from(big.clone()).try_repr().is_err());
            }
            let value = Value::from(big.clone());
            assert_eq!(value.as_int(), big.to_i64());
        }
        Err(ParseBigIntError::ExceedsMaxStrDigits { digits }) => {
            assert!(digits > BigInt::MAX_STR_DIGITS);
            assert!(!radix.is_power_of_two());
        }
        Err(_) => {}
    }
});
