"""Exceptions raised by typemind. All derive from `TypemindError`."""


class TypemindError(Exception):
    """Base class of every typemind error."""


class ConflictError(TypemindError):
    """A concurrent commit changed what this transaction depends on."""


class CorruptionError(TypemindError):
    """Stored data failed its checksum or structural validation."""


class NotFoundError(TypemindError, LookupError):
    """A requested object, version or index does not exist."""


class NamespaceNotFoundError(NotFoundError):
    """`name` is not a namespace; `available` lists those that exist."""

    def __init__(self, message, name=None, available=()):
        super().__init__(message)
        self.name = name
        self.available = list(available)


class LayerNotFoundError(NotFoundError):
    """Layer `name` has no index in `namespace` at `version`; `available` lists its indexes."""

    def __init__(self, message, name=None, namespace=None, version=None, available=()):
        super().__init__(message)
        self.name = name
        self.namespace = namespace
        self.version = version
        self.available = list(available)


class InvalidInputError(TypemindError, ValueError):
    """An argument or document was invalid."""


class StorageError(TypemindError, OSError):
    """The file system or object store failed."""
