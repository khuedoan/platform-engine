use anyhow::Result;

#[derive(Debug, Clone)]
pub struct GitOps {
    repo: String,
}

pub struct Namespace {
    name: String,
}

pub struct App {
    name: String,
    namespace: Namespace,
}

impl GitOps {
    pub async fn create_namespace(&self, name: String) -> Result<Namespace> {
        Ok(Namespace { name })
    }

    pub async fn get_namespace(&self, name: String) -> Result<Namespace> {
        Ok(Namespace { name })
    }

    pub async fn create_app(&self, namespace: Namespace, name: String) -> Result<App> {
        Ok(App { namespace, name })
    }

    pub async fn get_app(&self, namespace: Namespace, name: String) -> Result<App> {
        Ok(App { namespace, name })
    }
}
