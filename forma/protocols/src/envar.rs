use std::env::VarError;

use crate::FormaError;

/// Gets an environment variable as a `String`, returning `None` if the variable is not set.
pub fn get_env_var(key: &str) -> Result<Option<String>, FormaError> {
    let v = std::env::var(key);
    let v = match v {
        Ok(v) => Some(v),
        Err(VarError::NotPresent) => None,
        Err(e) => {
            return Err(FormaError::MissingEnvVar {
                key: key.to_string(),
                msg: e.to_string(),
            });
        }
    };
    Ok(v)
}

/// Gets an environment variable as a `String`, returning an error if the variable is not set.
pub fn expect_env_var(key: &str) -> Result<String, FormaError> {
    let v = get_env_var(key)?;
    if let Some(v) = v {
        Ok(v)
    } else {
        Err(FormaError::MissingEnvVar {
            key: key.to_string(),
            msg: "variable not set".to_string(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::FormaError;

    #[test]
    fn test_get_env_var_present() {
        let key = "TEST_ENVAR_GET_PRESENT_7391";
        unsafe { std::env::set_var(key, "hello") };
        let result = get_env_var(key).unwrap();
        assert_eq!(result, Some("hello".to_string()));
        unsafe { std::env::remove_var(key) };
    }

    #[test]
    fn test_get_env_var_error_carries_key() {
        let key = "TEST_ENVAR_KEY_IN_ERROR_7393";
        unsafe { std::env::remove_var(key) };
        let err = get_env_var(key).unwrap();
        assert!(err.is_none());
    }

    #[test]
    fn test_demand_env_var_present() {
        let key = "TEST_ENVAR_DEMAND_PRESENT_7394";
        unsafe { std::env::set_var(key, "world") };
        let result = expect_env_var(key).unwrap();
        assert_eq!(result, "world");
        unsafe { std::env::remove_var(key) };
    }

    #[test]
    fn test_demand_env_var_missing_returns_err() {
        let key = "TEST_ENVAR_DEMAND_MISSING_7395";
        unsafe { std::env::remove_var(key) };
        let err = expect_env_var(key).unwrap_err();
        assert!(matches!(err, FormaError::MissingEnvVar { .. }));
    }

    #[test]
    fn test_demand_env_var_error_carries_key() {
        let key = "TEST_ENVAR_DEMAND_KEY_7396";
        unsafe { std::env::remove_var(key) };
        let err = expect_env_var(key).unwrap_err();
        if let FormaError::MissingEnvVar { key: k, .. } = err {
            assert_eq!(k, key);
        } else {
            panic!("expected MissingEnvVar");
        }
    }
}
