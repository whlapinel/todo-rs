$version: "2"

namespace common

structure TeamMemberSummary {
    @required
    userId: String

    @required
    firstName: String

    @required
    lastName: String

    @required
    status: String
}

list TeamMembers {
    member: TeamMemberSummary
}

structure TeamSummary {
    @required
    teamId: String

    @required
    name: String

    @required
    status: String

    invitedByName: String
}

list Teams {
    member: TeamSummary
}

@http(method: "POST", uri: "/users/{userId}/teams")
operation CreateTeam {
    input := {
        @required
        @httpLabel
        userId: String

        @required
        @notProperty
        name: String
    }

    output := {
        @required
        @notProperty
        teamId: String
    }

    errors: [
        PeoplesRepublicOfListsError
    ]
}

@readonly
@http(method: "GET", uri: "/users/{userId}/teams/{teamId}")
operation GetTeam {
    input := {
        @required
        @httpLabel
        userId: String

        @required
        @httpLabel
        @notProperty
        teamId: String
    }

    output := {
        @required
        @notProperty
        teamId: String

        @required
        @notProperty
        name: String
    }

    errors: [
        PeoplesRepublicOfListsError
    ]
}

@readonly
@http(method: "GET", uri: "/users/{userId}/teams")
operation ListTeams {
    input := {
        @required
        @httpLabel
        userId: String
    }

    output := {
        @required
        @notProperty
        teams: Teams
    }

    errors: [
        PeoplesRepublicOfListsError
    ]
}

@readonly
@http(method: "GET", uri: "/users/{userId}/teams/{teamId}/members")
operation ListTeamMembers {
    input := {
        @required
        @httpLabel
        userId: String

        @required
        @httpLabel
        @notProperty
        teamId: String
    }

    output := {
        @required
        @notProperty
        members: TeamMembers
    }

    errors: [
        PeoplesRepublicOfListsError
    ]
}

@http(method: "POST", uri: "/users/{userId}/teams/{teamId}/invites")
operation InviteTeamMember {
    input := {
        @required
        @httpLabel
        userId: String

        @required
        @httpLabel
        @notProperty
        teamId: String

        @required
        @notProperty
        inviteeUserId: String
    }

    output := {}

    errors: [
        PeoplesRepublicOfListsError
    ]
}

@idempotent
@http(method: "PUT", uri: "/users/{userId}/teams/{teamId}/accept")
operation AcceptTeamInvite {
    input := {
        @required
        @httpLabel
        userId: String

        @required
        @httpLabel
        @notProperty
        teamId: String
    }

    output := {}

    errors: [
        PeoplesRepublicOfListsError
    ]
}

@idempotent
@http(method: "DELETE", uri: "/users/{userId}/teams/{teamId}/membership")
operation LeaveTeam {
    input := {
        @required
        @httpLabel
        userId: String

        @required
        @httpLabel
        @notProperty
        teamId: String
    }

    output := {}

    errors: [
        PeoplesRepublicOfListsError
    ]
}
