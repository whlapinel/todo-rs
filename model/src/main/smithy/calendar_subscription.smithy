$version: "2"

namespace common

// CalendarSubscription CRUD — Stage 5 of docs/google-calendar-import-plan.md. Scoped
// under /projects/{projectId}/calendar-subscriptions, no userId path segment — same
// precedent as ItemSeries (item_series.smithy) and Project's own
// ListProjectMembers/AttachTeamToProject (project.smithy): the acting user comes from
// the AuthUser extracted from the bearer token, not a path parameter. No Update
// operation — changing the URL is delete-and-recreate (see the plan's own Stage 5
// note: a subscription's identity/history isn't meaningful enough to warrant
// edit-in-place).
structure CalendarSubscriptionSummary {
    @required
    id: String

    @required
    projectId: String

    @required
    icalUrl: String

    @required
    createdByUserId: String

    @required
    createdAt: Timestamp

    lastSyncedAt: Timestamp

    lastSyncError: String
}

list CalendarSubscriptions {
    member: CalendarSubscriptionSummary
}

@http(method: "POST", uri: "/projects/{projectId}/calendar-subscriptions")
operation CreateCalendarSubscription {
    input := {
        @required
        @httpLabel
        projectId: String

        @required
        @notProperty
        icalUrl: String
    }

    output := {
        @required
        @notProperty
        id: String
    }

    errors: [
        PeoplesRepublicOfListsError
    ]
}

@readonly
@http(method: "GET", uri: "/projects/{projectId}/calendar-subscriptions")
operation ListCalendarSubscriptions {
    input := {
        @required
        @httpLabel
        projectId: String
    }

    output := {
        @required
        @notProperty
        subscriptions: CalendarSubscriptions
    }

    errors: [
        PeoplesRepublicOfListsError
    ]
}

@idempotent
@http(method: "DELETE", uri: "/projects/{projectId}/calendar-subscriptions/{id}")
operation DeleteCalendarSubscription {
    input := {
        @required
        @httpLabel
        projectId: String

        @required
        @httpLabel
        @notProperty
        id: String
    }

    output := {}

    errors: [
        PeoplesRepublicOfListsError
    ]
}
