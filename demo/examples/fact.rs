extern crate stainless;
use stainless::*;

#[pre(x >= 0)]
#[post(ret >= 0)]
pub fn fact(x: i32) -> i32 {
  if x <= 0 {
    1
  } else {
    fact(x - 1) * x
  }
}

fn main() -> () {}
