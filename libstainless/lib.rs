pub use num_bigint::BigInt;
pub use stainless_macros::*;

use std::marker::PhantomData;

/// This type is a placeholder for some methods with the same names as those in
/// Set<T>. It shows that the std item lookup can't check whether it extracts
/// the methods on the correct implementation. This would be needed for
/// extracting `Box::new` though.
pub struct TestType;

impl TestType {
  pub fn empty() -> Self {
    unimplemented!()
  }
  pub fn singleton<T>(_t: T) -> Self {
    unimplemented!()
  }
  pub fn add<T>(self, _t: T) -> TestType {
    unimplemented!()
  }
}

/// Stainless' set type, useful for proofs. There are no runtime implementations
/// for the methods, though.
#[derive(Copy, Clone, PartialEq)]
pub struct Set<T> {
  phantom: PhantomData<T>,
}

impl<T> Set<T> {
  pub fn empty() -> Self {
    unimplemented!()
  }

  // TODO: Only take 'self' as a reference and also take the other parameters
  //   only by reference.
  pub fn singleton(_t: &T) -> Self {
    unimplemented!()
  }
  pub fn add(&self, _t: &T) -> Set<T> {
    unimplemented!()
  }
  pub fn contains(&self, _t: &T) -> bool {
    unimplemented!()
  }

  pub fn union(&self, _other: &Set<T>) -> Set<T> {
    unimplemented!()
  }
  pub fn intersection(&self, _other: &Set<T>) -> Set<T> {
    unimplemented!()
  }
  pub fn difference(&self, _other: &Set<T>) -> Set<T> {
    unimplemented!()
  }
  pub fn is_subset_of(&self, _other: &Set<T>) -> bool {
    unimplemented!()
  }
}
