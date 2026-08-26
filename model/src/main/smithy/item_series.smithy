$version: "2"

namespace common

// ItemSeries CRUD — stage 4a of docs/recurring-events-virtual-occurrences-rough-plan.md's
// staged breakdown (as EventSeries, Event-only). Renamed EventSeries -> ItemSeries and
// gained a required itemType field at stage 7b, once the storage/domain layer (stage 7a)
// already supported Task-typed series. Scoped under /projects/{projectId}/series, no
// userId path segment — mirrors project.smithy's ListProjectMembers/AttachTeamToProject
// precedent, where the acting user comes from the AuthUser extracted from the bearer
// token, not a path parameter.
//
// name/recurrence/anchorDate/itemType are required on both Create and Update
// (full-replace, no partial-update semantics — matches UpdateProject's always-required
// `name`). itemType has no server-side default: every caller must be explicit about
// whether a series materializes Task or Event occurrences. description/eventType are
// optional, direct-overwrite fields (Update always round-trips them; omitting one clears
// it) — same convention Item's own optional fields (dueDate, eventType, etc.) already
// follow.
//
// DeleteItemSeries (added 2026-08-15) resolves the deferred question above as an orphan,
// not a cascade: it deletes the item_series row and all its item_occurrences rows, but
// never touches the items table. Every already-materialized occurrence survives as a
// plain standalone item — same treatment `unlink_source_event_tasks` already gives a
// deleted Event's linked tasks (CLAUDE.md's Events section), chosen for consistency with
// that precedent over the structural-children cascade `delete_item`/`delete_team_item`
// use, since a materialized occurrence is a full standalone item with its own
// completion/points history, not a structural child of the series. Gated by project
// membership, matching Create/Update/List above.
//
// docs/series-sub-items-plan.md (stage 1, 2026-08-26): removed `templateItemId` outright
// (no migration/backfill — confirmed nothing depended on it) and added `parentSeriesId`/
// `dueOffsetDays`, letting a Task-typed series be a sub-item of another Task-typed series.
// Validation (only a Task series may set `parentSeriesId`, one level of nesting only, etc.)
// lands in a later stage — these two fields are unvalidated on the wire for now. A child
// series will eventually have no `recurrence`/`anchorDate` of its own (inherited from its
// parent), but those two fields stay `@required` here until that stage relaxes them.
structure ItemSeriesSummary {
    @required
    seriesId: String

    @required
    projectId: String

    @required
    name: String

    description: String

    eventType: String

    @required
    recurrence: String

    @required
    anchorDate: Timestamp

    @required
    itemType: ItemType

    basis: String

    parentSeriesId: String

    dueOffsetDays: Integer

    assignedToUserId: String

    rotationUserIds: StringList

    points: Integer
}

list ItemSeriesList {
    member: ItemSeriesSummary
}

// docs/assignment-rotation-plan.md's rotating-assignee feature: `assignedToUserId` and
// `rotationUserIds` are mutually exclusive per series (enforced service-side, not here —
// Smithy has no clean "exactly one of" constraint), and stay present on the wire
// simultaneously so a client always sees which mode is active by which one is populated.
list StringList {
    member: String
}

@http(method: "POST", uri: "/projects/{projectId}/series")
operation CreateItemSeries {
    input := {
        @required
        @httpLabel
        projectId: String

        @required
        @notProperty
        name: String

        @notProperty
        description: String

        @notProperty
        eventType: String

        @required
        @notProperty
        recurrence: String

        @required
        @notProperty
        anchorDate: Timestamp

        @required
        @notProperty
        itemType: ItemType

        @notProperty
        basis: String

        @notProperty
        parentSeriesId: String

        @notProperty
        dueOffsetDays: Integer

        @notProperty
        assignedToUserId: String

        @notProperty
        rotationUserIds: StringList

        @notProperty
        points: Integer
    }

    output := {
        @required
        @notProperty
        seriesId: String
    }

    errors: [
        PeoplesRepublicOfListsError
    ]
}

@readonly
@http(method: "GET", uri: "/projects/{projectId}/series/{seriesId}")
operation GetItemSeries {
    input := {
        @required
        @httpLabel
        projectId: String

        @required
        @httpLabel
        @notProperty
        seriesId: String
    }

    output := {
        @required
        @notProperty
        seriesId: String

        @required
        @notProperty
        projectId: String

        @required
        @notProperty
        name: String

        @notProperty
        description: String

        @notProperty
        eventType: String

        @required
        @notProperty
        recurrence: String

        @required
        @notProperty
        anchorDate: Timestamp

        @required
        @notProperty
        itemType: ItemType

        @notProperty
        basis: String

        @notProperty
        parentSeriesId: String

        @notProperty
        dueOffsetDays: Integer

        @notProperty
        assignedToUserId: String

        @notProperty
        rotationUserIds: StringList

        @notProperty
        points: Integer
    }

    errors: [
        PeoplesRepublicOfListsError
    ]
}

@idempotent
@http(method: "PUT", uri: "/projects/{projectId}/series/{seriesId}")
operation UpdateItemSeries {
    input := {
        @required
        @httpLabel
        projectId: String

        @required
        @httpLabel
        @notProperty
        seriesId: String

        @required
        @notProperty
        name: String

        @notProperty
        description: String

        @notProperty
        eventType: String

        @required
        @notProperty
        recurrence: String

        @required
        @notProperty
        anchorDate: Timestamp

        @required
        @notProperty
        itemType: ItemType

        @notProperty
        basis: String

        @notProperty
        parentSeriesId: String

        @notProperty
        dueOffsetDays: Integer

        @notProperty
        assignedToUserId: String

        @notProperty
        rotationUserIds: StringList

        @notProperty
        points: Integer
    }

    output := {}

    errors: [
        PeoplesRepublicOfListsError
    ]
}

@idempotent
@http(method: "DELETE", uri: "/projects/{projectId}/series/{seriesId}")
operation DeleteItemSeries {
    input := {
        @required
        @httpLabel
        projectId: String

        @required
        @httpLabel
        @notProperty
        seriesId: String
    }

    output := {}

    errors: [
        PeoplesRepublicOfListsError
    ]
}

@readonly
@http(method: "GET", uri: "/projects/{projectId}/series")
operation ListItemSeriesForProject {
    input := {
        @required
        @httpLabel
        projectId: String
    }

    output := {
        @required
        @notProperty
        series: ItemSeriesList
    }

    errors: [
        PeoplesRepublicOfListsError
    ]
}
