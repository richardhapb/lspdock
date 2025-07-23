use std::{env::current_dir, ffi::OsStr, ops::Deref};

use crate::config::ProxyConfig;

pub trait VariableResolver {
    fn expand(self, config: &mut ProxyConfig) -> Result<(), Box<dyn std::error::Error>>;
}

/// Base struct for variable resolvers that have a simple name field
pub(super) struct Variable {
    name: String,
}

impl Variable {
    fn new(name: &str) -> Self {
        Self { name: name.into() }
    }
}

impl Deref for Variable {
    type Target = str;

    fn deref(&self) -> &Self::Target {
        &self.name
    }
}

/// Current working directory variable
pub(super) struct VariableCwd(Variable);

impl Default for VariableCwd {
    fn default() -> Self {
        Self(Variable::new("CWD"))
    }
}

/// Parent directory variable
pub(super) struct VariableParent(Variable);

impl Default for VariableParent {
    fn default() -> Self {
        Self(Variable::new("PARENT"))
    }
}

impl VariableResolver for VariableCwd {
    fn expand(self, config: &mut ProxyConfig) -> Result<(), Box<dyn std::error::Error>> {
        let var = format!("${}", self.0.name.to_uppercase());
        let cwd = current_dir()?;
        let expanded = cwd
            .to_str()
            .ok_or_else(|| "Could not convert current directory to string".to_string())?;
        expand_into_config(config, &var, &expanded);

        Ok(())
    }
}

impl VariableResolver for VariableParent {
    fn expand(self, config: &mut ProxyConfig) -> Result<(), Box<dyn std::error::Error>> {
        let var = format!("${}", self.0.name.to_uppercase());
        let cwd = current_dir()?;
        let parent = cwd.file_name().unwrap_or(OsStr::new(""));
        let expanded = parent
            .to_str()
            .ok_or_else(|| "Could not convert current directory to string".to_string())?;
        expand_into_config(config, &var, &expanded);

        Ok(())
    }
}

fn expand_into_config(config: &mut ProxyConfig, var: &str, expanded: &str) {
    let fields = [
        &mut config.container,
        &mut config.local_path,
        &mut config.executable,
        &mut config.docker_internal_path,
        &mut config.pattern,
    ];

    for field in fields {
        *field = field.replace(var, expanded);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn variable_expand() {
        let par_var = VariableParent::default();
        let cwd_var = VariableCwd::default();
        let mut config = ProxyConfig {
            container: "$PARENT-web-1".into(),
            local_path: "$CWD/app".into(),
            docker_internal_path: "/some/path".into(),
            pattern: "/some/pattern".into(),
            executable: "rust_analyzer".into(),
            use_docker: false,
        };

        par_var.expand(&mut config).unwrap();
        cwd_var.expand(&mut config).unwrap();

        let cwd = current_dir().unwrap();
        let parent = cwd.file_name().unwrap();
        let parent = parent.to_str().unwrap();
        let cwd = cwd.to_str().unwrap();

        assert_eq!(config.container, format!("{parent}-web-1"));
        assert_eq!(config.local_path, format!("{cwd}/app"));
    }
}
