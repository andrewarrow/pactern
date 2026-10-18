//! Conventional Rust implementations; no Pactern evaluator or expected-value lookup.
use serde::Deserialize;
use serde_json::{Value, json};
use std::io::{self, Read};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Request {
    id: String,
    task: String,
    args: Vec<Value>,
}

fn integer(value: &Value) -> i64 {
    value
        .as_str()
        .expect("integer string")
        .parse()
        .expect("I64")
}

fn returned(value: Value) -> Value {
    json!({"kind": "return", "value": value})
}

fn trap(category: &str) -> Value {
    json!({"kind": "trap", "category": category})
}

fn reserve(available: i64, amount: i64) -> Result<i64, &'static str> {
    if amount <= 0 {
        return Err("InvalidAmount");
    }
    if amount > available {
        return Err("InsufficientQuota");
    }
    Ok(available - amount)
}

fn reserve_safety(available: i64, amount: i64, result: Result<i64, &str>) -> bool {
    match result {
        Ok(remaining) => {
            amount > 0
                && amount <= available
                && Some(remaining) == available.checked_sub(amount)
                && remaining >= 0
        }
        Err("InvalidAmount") => amount <= 0,
        Err("InsufficientQuota") => amount > 0 && amount > available,
        Err(_) => false,
    }
}

fn execute(request: Request) -> Value {
    let args = &request.args;
    let mut response = json!({"id": request.id});
    response["outcome"] = match request.task.as_str() {
        "quota" => {
            assert_eq!(args.len(), 2);
            let available = integer(&args[0]);
            let amount = integer(&args[1]);
            if available < 0 {
                response["pacts"] = json!({"precondition": "fail", "postcondition": "not_reached"});
                trap("precondition")
            } else {
                // Err is a normal result and must still satisfy the postcondition.
                let result = reserve(available, amount);
                if reserve_safety(available, amount, result) {
                    response["pacts"] = json!({"precondition": "pass", "postcondition": "pass"});
                    returned(match result {
                        Ok(remaining) => json!({"tag": "Ok", "value": remaining.to_string()}),
                        Err(error) => json!({"tag": "Err", "error": error}),
                    })
                } else {
                    response["pacts"] = json!({"precondition": "pass", "postcondition": "fail"});
                    trap("postcondition")
                }
            }
        }
        "pair_totals" => {
            assert_eq!(args.len(), 1);
            let values = args[0].as_array().expect("list");
            // Preserve source order: an overflow in the first sum precedes
            // an out-of-bounds access in the second sum.
            let pair = |start: usize| -> Result<i64, &str> {
                let left = values.get(start).ok_or("bounds")?;
                let right = values.get(start + 1).ok_or("bounds")?;
                integer(left)
                    .checked_add(integer(right))
                    .ok_or("integer_overflow")
            };
            match pair(0).and_then(|first| pair(2).map(|second| (first, second))) {
                Ok((first, second)) => returned(json!([first.to_string(), second.to_string()])),
                Err(category) => trap(category),
            }
        }
        "summarize" => {
            assert_eq!(args.len(), 3);
            let label = args[0].as_str().expect("label");
            match integer(&args[1]).checked_add(integer(&args[2])) {
                Some(total) => returned(
                    json!({"label": label, "total": total.to_string(), "active": total > 0}),
                ),
                None => trap("integer_overflow"),
            }
        }
        other => panic!("unknown task: {other}"),
    };
    response
}

fn main() {
    let mut input = String::new();
    io::stdin()
        .read_to_string(&mut input)
        .expect("read requests");
    let requests: Vec<Request> = serde_json::from_str(&input).expect("request JSON");
    let results: Vec<_> = requests.into_iter().map(execute).collect();
    println!(
        "{}",
        serde_json::to_string(&results).expect("response JSON")
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn guard_rejects_unchecked_subtraction_and_reject_all_repairs() {
        assert!(!reserve_safety(3, 4, Ok(-1)));
        assert!(!reserve_safety(3, 0, Ok(3)));
        assert!(!reserve_safety(3, 2, Err("InvalidAmount")));
        assert!(!reserve_safety(3, 4, Err("InvalidAmount")));
        assert!(reserve_safety(i64::MAX, i64::MAX, Ok(0)));
        assert!(reserve_safety(3, i64::MIN, Err("InvalidAmount")));
    }
}
