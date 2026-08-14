$version: "2"

namespace common

// EventSeries CRUD — stage 4a of
// docs/recurring-events-virtual-occurrences-rough-plan.md's staged breakdown. Scoped
// under /projects/{projectId}/series, no userId path segment — mirrors project.smithy's
// ListProjectMembers/AttachTeamToProject precedent, where the acting user comes from
// the AuthUser extracted from the bearer token, not a path parameter.
//
// name/recurrence/anchorDate are required on both Create and Update (full-replace, no
// partial-update semantics — matches UpdateProject's always-required `name`).
// description/eventType are optional, direct-overwrite fields (Update always
// round-trips them; omitting one clears it) — same convention Item's own optional
// fields (dueDate, eventType, etc.) already follow.
//
// No DeleteEventSeries yet — deferred, since deleting a series with already-
// materialized occurrences raises the same open question stage 6 already defers for
// skipping one, not answered here.
structure EventSeriesSummary {
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
}

list EventSeriesList {
    member: EventSeriesSummary
}

@http(method: "POST", uri: "/projects/{projectId}/series")
operation CreateEventSeries {
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
operation GetEventSeries {
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
    }

    errors: [
        PeoplesRepublicOfListsError
    ]
}

@idempotent
@http(method: "PUT", uri: "/projects/{projectId}/series/{seriesId}")
operation UpdateEventSeries {
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
    }

    output := {}

    errors: [
        PeoplesRepublicOfListsError
    ]
}

@readonly
@http(method: "GET", uri: "/projects/{projectId}/series")
operation ListEventSeriesForProject {
    input := {
        @required
        @httpLabel
        projectId: String
    }

    output := {
        @required
        @notProperty
        series: EventSeriesList
    }

    errors: [
        PeoplesRepublicOfListsError
    ]
}
