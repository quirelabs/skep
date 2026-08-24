use crate::id::InstanceId;
use crate::state::ServiceState;

pub type Result<T, E = Error> = std::result::Result<T, E>;

#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum Error {
    #[error("invalid id: {0}")]
    InvalidId(String),

    #[error("no instance {0} is registered")]
    UnknownInstance(InstanceId),

    #[error("instance {0} is already registered")]
    AlreadyRegistered(InstanceId),

    #[error("{instance} cannot go from {from} to {to}")]
    IllegalTransition {
        instance: InstanceId,
        from: ServiceState,
        to: ServiceState,
    },

    #[error("{instance} is {state}, it must be stopped first")]
    NotStopped {
        instance: InstanceId,
        state: ServiceState,
    },

    #[error("{0} is not implemented yet")]
    NotImplemented(&'static str),

    #[error(transparent)]
    Io(#[from] std::io::Error),
}
