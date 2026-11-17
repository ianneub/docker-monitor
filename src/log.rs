//! Every line on stdout is a single JSON object. `serde_json`'s `preserve_order`
//! feature keeps keys in the order written, matching the Ruby version's output.

use serde_json::Value;

pub fn emit(value: Value) {
    println!("{value}");
}
