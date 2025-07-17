use thiserror::Error;

#[derive(Error, Clone, Debug, Eq, PartialEq)]
pub enum Curve25519Error {
    #[error("pod conversion failed")]
    PodConversion,
}

impl From<core::array::TryFromSliceError> for Curve25519Error {
    fn from(_: core::array::TryFromSliceError) -> Self {
        Curve25519Error::PodConversion
    }
}
