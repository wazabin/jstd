use jstd::registry::Identifier;

/// A generic wrapper struct for referencing an ID with a context.
/// This is the basis for all `*Ref` and `*MutRef`
pub struct BaseRef<Ctx, Id: Identifier> {
    pub id: Id,
    pub ctx: Ctx,
}

impl<Ctx, Id: Identifier> BaseRef<Ctx, Id> {
    pub fn new(ctx: Ctx, id: Id) -> Self {
        BaseRef { id, ctx }
    }
}

impl<Ctx, Id: Identifier> BaseRef<Ctx, Id> {
    pub fn from_id(ctx: Ctx, id: Id) -> Self {
        BaseRef { id, ctx }
    }
}

/// Convert mutable refs to immutable refs by cloning the ID and sharing the context reference.
impl<'ctx, Ctx, Id: Identifier> From<BaseRef<&'ctx mut Ctx, Id>> for BaseRef<&'ctx Ctx, Id> {
    fn from(r: BaseRef<&'ctx mut Ctx, Id>) -> Self {
        BaseRef {
            id: r.id,
            ctx: r.ctx,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::BaseRef;

    #[test]
    fn constructors_keep_context_and_identifier() {
        let reference = BaseRef::new("context", 7_usize);
        let from_id = BaseRef::from_id("other", 9_usize);

        assert_eq!(reference.ctx, "context");
        assert_eq!(reference.id, 7);
        assert_eq!(from_id.ctx, "other");
        assert_eq!(from_id.id, 9);
    }

    #[test]
    fn mutable_context_reference_converts_to_shared_reference() {
        let mut context = String::from("context");
        let mutable = BaseRef::new(&mut context, 3_usize);
        let shared: BaseRef<&String, usize> = mutable.into();

        assert_eq!(shared.id, 3);
        assert_eq!(shared.ctx, "context");
    }
}
