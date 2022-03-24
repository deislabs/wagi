use crate::dispatcher::RoutePattern;

pub struct DynamicRoutes {
    // Using a Vec rather than a HashMap because order matters
    // (and direct lookup doesn't because some routes may be prefixes)
    pub subpath_entrypoints: Vec<(RoutePattern, String)>, // TODO: private
}

pub fn interpret_routes(route_text: impl Into<String>) -> anyhow::Result<DynamicRoutes> {
    let route_text = route_text.into();
    if route_text.is_empty() {
        return Err(anyhow::anyhow!("Dynamic routes text was empty"));
    }

    let routes = route_text
        .lines()
        .filter(|s| !s.is_empty())
        .map(parse_dynamic_route)
        .collect::<Result<Vec<_>, _>>()?;
    Ok(DynamicRoutes {
        subpath_entrypoints: routes,
    })
}

fn parse_dynamic_route(line: &str) -> anyhow::Result<(RoutePattern, String)> {
    let parts: Vec<&str> = line.trim().split_whitespace().collect();

    if parts.is_empty() {
        return Err(anyhow::anyhow!("Dynamic routes contained empty line"));
    }
    if parts.len() != 2 {
        return Err(anyhow::anyhow!(
            "Dynamic routes contained invalid line {}",
            line
        ));
    }

    let path_text = parts.get(0).unwrap_or(&"/");
    let entrypoint = parts.get(1).unwrap_or(&"_start").to_string();

    let route_pattern = RoutePattern::parse(path_text);
    Ok((route_pattern, entrypoint))
}

#[cfg(test)]
mod test {
    use super::*;

    #[test]
    pub fn empty_route_map_is_error() {
        assert!(interpret_routes("").is_err());
    }

    #[test]
    pub fn route_map_with_one_column_is_error() {
        assert!(interpret_routes("/hello").is_err());
        assert!(interpret_routes("/hello hello\n/goodbye").is_err());
    }

    #[test]
    pub fn route_map_with_three_columns_is_error() {
        assert!(interpret_routes("/hello hello heya").is_err());
        assert!(interpret_routes("/hello hello\n/goodbye goodbye and_farewell").is_err());
    }

    #[test]
    pub fn route_map_with_two_columns_is_ok() {
        assert!(interpret_routes("/hello hello").is_ok());
        assert!(interpret_routes("/hello hello\n").is_ok());
        assert!(interpret_routes("/hello hello\n/goodbye goodbye").is_ok());
    }

    #[test]
    pub fn can_parse_plain_routes() {
        let routes = interpret_routes("/hello hello\n/goodbye farewell").unwrap();
        let entrypoints = routes.subpath_entrypoints;

        assert_eq!(2, entrypoints.len());

        assert_eq!(RoutePattern::Exact("/hello".to_owned()), entrypoints[0].0);
        assert_eq!("hello", entrypoints[0].1);
        assert_eq!(RoutePattern::Exact("/goodbye".to_owned()), entrypoints[1].0);
        assert_eq!("farewell", entrypoints[1].1);
    }

    #[test]
    pub fn can_parse_wildcard_routes() {
        let routes = interpret_routes("/hello/... hello\n/goodbye/... au_revoir").unwrap();
        let entrypoints = routes.subpath_entrypoints;

        assert_eq!(2, entrypoints.len());

        assert_eq!(RoutePattern::Prefix("/hello".to_owned()), entrypoints[0].0);
        assert_eq!("hello", entrypoints[0].1);
        assert_eq!(
            RoutePattern::Prefix("/goodbye".to_owned()),
            entrypoints[1].0
        );
        assert_eq!("au_revoir", entrypoints[1].1);
    }
}
