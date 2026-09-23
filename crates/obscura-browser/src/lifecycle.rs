#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LifecycleState {
    Idle,
    Loading,
    DomContentLoaded,
    Loaded,
    NetworkAlmostIdle,
    NetworkIdle,
    Failed,
}

impl LifecycleState {
    pub fn is_loading(&self) -> bool {
        matches!(self, LifecycleState::Loading)
    }

    pub fn is_loaded(&self) -> bool {
        matches!(
            self,
            LifecycleState::Loaded
                | LifecycleState::NetworkAlmostIdle
                | LifecycleState::NetworkIdle
        )
    }

    pub fn is_network_almost_idle(&self) -> bool {
        matches!(self, LifecycleState::NetworkAlmostIdle | LifecycleState::NetworkIdle)
    }

    pub fn is_network_idle(&self) -> bool {
        matches!(self, LifecycleState::NetworkIdle)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WaitUntil {
    Load,
    DomContentLoaded,
    NetworkIdle0,
    NetworkIdle2,
}

pub const NETWORK_IDLE_QUIET_WINDOW: std::time::Duration =
    std::time::Duration::from_millis(500);

impl WaitUntil {
    pub fn network_idle_threshold(self) -> Option<u32> {
        match self {
            Self::NetworkIdle0 => Some(0),
            Self::NetworkIdle2 => Some(2),
            Self::Load | Self::DomContentLoaded => None,
        }
    }
}

impl WaitUntil {
    pub fn from_str(s: &str) -> Self {
        match s {
            "domcontentloaded" => WaitUntil::DomContentLoaded,
            "networkidle0" | "networkIdle" | "networkidle" => WaitUntil::NetworkIdle0,
            "networkidle2" => WaitUntil::NetworkIdle2,
            _ => WaitUntil::Load,
        }
    }
}
