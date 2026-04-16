use std::cell::RefCell;
use std::rc::Rc;

thread_local! {
    static GLOBAL_APP_STATE: RefCell<Option<AppOperationState>> = const { RefCell::new(None) };
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct OperationState {
    pub install_active: bool,
    pub deploy_active: bool,
    pub runtime_session_active: bool,
    pub message: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LockPolicy {
    pub allow_game_switch: bool,
    pub allow_profile_switch: bool,
    pub allow_deploy_rebuild: bool,
    pub allow_reorder: bool,
    pub allow_destructive_mutations: bool,
}

impl LockPolicy {
    pub fn is_read_only(&self) -> bool {
        !(self.allow_deploy_rebuild && self.allow_reorder && self.allow_destructive_mutations)
    }
}

pub fn derive_lock_policy(state: &OperationState) -> LockPolicy {
    let runtime_lock = state.runtime_session_active;
    let install_lock = state.install_active;
    let deploy_lock = state.deploy_active;
    let block_mutations = runtime_lock || install_lock || deploy_lock;
    LockPolicy {
        allow_game_switch: !block_mutations,
        allow_profile_switch: !block_mutations,
        allow_deploy_rebuild: !block_mutations,
        allow_reorder: !block_mutations,
        allow_destructive_mutations: !block_mutations,
    }
}

type Observer = Box<dyn Fn(&OperationState)>;

#[derive(Clone, Default)]
pub struct AppOperationState {
    inner: Rc<RefCell<OperationState>>,
    observers: Rc<RefCell<Vec<Observer>>>,
}

impl AppOperationState {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn snapshot(&self) -> OperationState {
        self.inner.borrow().clone()
    }

    pub fn subscribe(&self, observer: impl Fn(&OperationState) + 'static) {
        self.observers.borrow_mut().push(Box::new(observer));
    }

    pub fn update(&self, f: impl FnOnce(&mut OperationState)) {
        let snapshot = {
            let mut state = self.inner.borrow_mut();
            f(&mut state);
            state.clone()
        };
        for obs in self.observers.borrow().iter() {
            obs(&snapshot);
        }
    }
}

pub fn register_global_app_state(state: &AppOperationState) {
    GLOBAL_APP_STATE.with(|cell| {
        *cell.borrow_mut() = Some(state.clone());
    });
}

pub fn global_state_snapshot() -> OperationState {
    GLOBAL_APP_STATE
        .with(|cell| cell.borrow().as_ref().map(AppOperationState::snapshot))
        .unwrap_or_default()
}

pub fn update_global_state(f: impl FnOnce(&mut OperationState)) {
    GLOBAL_APP_STATE.with(|cell| {
        if let Some(state) = cell.borrow().as_ref() {
            state.update(f);
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn runtime_active_blocks_mutations_but_not_observability_pages() {
        let state = OperationState {
            runtime_session_active: true,
            ..OperationState::default()
        };
        let policy = derive_lock_policy(&state);
        assert!(!policy.allow_game_switch);
        assert!(!policy.allow_profile_switch);
        assert!(!policy.allow_deploy_rebuild);
        assert!(!policy.allow_reorder);
        assert!(!policy.allow_destructive_mutations);
        assert!(policy.is_read_only());
    }

    #[test]
    fn unlocked_state_allows_actions() {
        let policy = derive_lock_policy(&OperationState::default());
        assert!(policy.allow_game_switch);
        assert!(policy.allow_profile_switch);
        assert!(policy.allow_deploy_rebuild);
        assert!(policy.allow_reorder);
        assert!(policy.allow_destructive_mutations);
        assert!(!policy.is_read_only());
    }

    #[test]
    fn app_state_notifies_observers_after_update() {
        let state = AppOperationState::new();
        let seen = Rc::new(RefCell::new(OperationState::default()));
        let seen_c = Rc::clone(&seen);
        state.subscribe(move |snapshot| {
            *seen_c.borrow_mut() = snapshot.clone();
        });
        state.update(|s| {
            s.install_active = true;
            s.message = Some("busy".to_string());
        });
        let got = seen.borrow().clone();
        assert!(got.install_active);
        assert_eq!(got.message.as_deref(), Some("busy"));
    }
}
