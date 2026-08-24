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
