use crate::Session;
#[derive(Default)]
pub struct SessionPreparationOptions {
    pub release: Option<Box<dyn FnOnce() + Send>>,
}
pub struct SessionPreparation {
    pub session: Session,
    release: Option<Box<dyn FnOnce() + Send>>,
}
impl SessionPreparation {
    pub fn create(session: Session, options: SessionPreparationOptions) -> Self {
        Self {
            session,
            release: options.release,
        }
    }
    pub fn dispose(&mut self) {
        if let Some(release) = self.release.take() {
            release();
        }
    }
}
impl Drop for SessionPreparation {
    fn drop(&mut self) {
        self.dispose();
    }
}
