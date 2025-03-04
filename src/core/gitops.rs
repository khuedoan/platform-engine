use anyhow::Result;

#[derive(Debug, Clone)]
pub struct GitOps {}

pub struct Namespace {}

pub struct App {}

impl GitOps {
    pub async fn create_namespace(&self, _name: String) -> Result<Namespace> {
        Ok(Namespace {})
    }

    pub async fn get_namespace(&self, _name: String) -> Result<Namespace> {
        Ok(Namespace {})
    }

    pub async fn create_app(&self, _namespace: Namespace, _name: String) -> Result<App> {
        Ok(App {})
    }

    pub async fn get_app(&self, _namespace: Namespace, _name: String) -> Result<App> {
        Ok(App {})
    }
}
