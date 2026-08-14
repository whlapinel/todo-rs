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
// No DeleteItemSeries yet — deferred, since deleting a series with already-materialized
// occurrences raises the same open question stage 6 already defers for skipping one, not
// answered here.
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
}

list ItemSeriesList {
    member: ItemSeriesSummary
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
