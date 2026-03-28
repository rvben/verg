use crate::error::Error;

#[derive(Debug, Clone, PartialEq)]
pub enum Selector {
    All,
    Group(String),
    Exclude(Box<Selector>),
    Union(Vec<Selector>),
    Intersection(Vec<Selector>),
}

pub fn parse_selector(input: &str) -> Result<Selector, Error> {
    let input = input.trim();
    if input.is_empty() {
        return Err(Error::Config("empty target selector".into()));
    }
    if input == "all" {
        return Ok(Selector::All);
    }
    if input.contains(',') {
        let parts: Result<Vec<_>, _> = input.split(',').map(|s| parse_selector(s.trim())).collect();
        return Ok(Selector::Union(parts?));
    }
    if input.contains(':') {
        let parts: Result<Vec<_>, _> = input.split(':').map(|s| parse_selector(s.trim())).collect();
        return Ok(Selector::Intersection(parts?));
    }
    if let Some(rest) = input.strip_prefix('!') {
        return Ok(Selector::Exclude(Box::new(parse_selector(rest)?)));
    }
    Ok(Selector::Group(input.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_all() {
        assert_eq!(parse_selector("all").unwrap(), Selector::All);
    }

    #[test]
    fn parse_single_name() {
        assert_eq!(
            parse_selector("web").unwrap(),
            Selector::Group("web".into())
        );
    }

    #[test]
    fn parse_union() {
        let sel = parse_selector("web1,web2").unwrap();
        assert_eq!(
            sel,
            Selector::Union(vec![
                Selector::Group("web1".into()),
                Selector::Group("web2".into()),
            ])
        );
    }

    #[test]
    fn parse_intersection() {
        let sel = parse_selector("prod:web").unwrap();
        assert_eq!(
            sel,
            Selector::Intersection(vec![
                Selector::Group("prod".into()),
                Selector::Group("web".into()),
            ])
        );
    }

    #[test]
    fn parse_exclusion() {
        let sel = parse_selector("prod:!web").unwrap();
        assert_eq!(
            sel,
            Selector::Intersection(vec![
                Selector::Group("prod".into()),
                Selector::Exclude(Box::new(Selector::Group("web".into()))),
            ])
        );
    }

    #[test]
    fn empty_selector_errors() {
        assert!(parse_selector("").is_err());
    }
}
