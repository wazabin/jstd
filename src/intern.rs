use rustc_hash::FxHashSet as HashSet;

/// Interns strings, returning 'static lifetime strings
#[derive(Default)]
pub struct StringPool(HashSet<&'static str>);

impl StringPool {
    /// Move a string into the pool
    pub fn intern(&mut self, s: &str) -> &'static str {
        if let Some(existing) = self.0.get(s) {
            return existing;
        }

        let boxed = s.to_owned().into_boxed_str();
        let leaked: &'static str = Box::leak(boxed);

        self.0.insert(leaked);
        leaked
    }
}

/// Moves a string into the pool, giving it a 'static lifetime
pub trait Intern {
    type Static;

    fn intern(self, pool: &mut StringPool) -> Self::Static;
}

impl Intern for &str {
    type Static = &'static str;

    fn intern(self, pool: &mut StringPool) -> Self::Static {
        pool.intern(self)
    }
}

impl<T> Intern for Vec<T>
where
    T: Intern,
{
    type Static = Vec<T::Static>;

    fn intern(self, pool: &mut StringPool) -> Self::Static {
        self.into_iter().map(|e| Intern::intern(e, pool)).collect()
    }
}

impl<T> Intern for Option<T>
where
    T: Intern,
{
    type Static = Option<T::Static>;

    fn intern(self, pool: &mut StringPool) -> Self::Static {
        self.map(|e| Intern::intern(e, pool))
    }
}
