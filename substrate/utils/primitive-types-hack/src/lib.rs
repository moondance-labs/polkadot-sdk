use primitive_types_a::U256 as OldU256;
use primitive_types_b::U256 as NewU256;
use primitive_types_a::H160 as OldH160;
use primitive_types_b::H160 as NewH160;
use primitive_types_a::H256 as OldH256;
use primitive_types_b::H256 as NewH256;

pub trait FromHack<T> {
    fn from(x: T) -> Self;
}

impl FromHack<NewU256> for OldU256 {
    fn from(x: NewU256) -> Self {
        todo!()
    }
}

impl FromHack<OldU256> for NewU256 {
    fn from(x: OldU256) -> Self {
        todo!()
    }
}

impl FromHack<NewH256> for OldH256 {
    fn from(x: NewH256) -> Self {
        todo!()
    }
}

impl FromHack<OldH256> for NewH256 {
    fn from(x: OldH256) -> Self {
        todo!()
    }
}

impl FromHack<NewH160> for OldH160 {
    fn from(x: NewH160) -> Self {
        todo!()
    }
}

impl FromHack<OldH160> for NewH160 {
    fn from(x: OldH160) -> Self {
        todo!()
    }
}

pub trait IntoHack<T> {
    fn into_p(self) -> T;
}

impl<A, B: FromHack<A>> IntoHack<B> for A {
    fn into_p(self) -> B {
        B::from(self)
    }
}
