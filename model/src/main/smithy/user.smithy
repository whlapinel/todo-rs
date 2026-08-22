$version: "2"

namespace common

resource User {
    identifiers: {
        userId: String
    }
    properties: {
        firstName: String
        lastName: String
        timezone: String
    }
    read: GetUser
    list: ListUsers
    update: UpdateUser
    operations: [
        ListItemsDue
        ListAssignedItems
        CreateTemplate
        ListTemplates
        CreateTeam
        GetTeam
        UpdateTeam
        ListTeams
        ListTeamMembers
        InviteTeamMember
        AcceptTeamInvite
        LeaveTeam
        SetTeamMemberRole
        SendAppInvite
        CreateProject
        GetProject
        UpdateProject
        DeleteProject
        ListProjects
    ]
}

structure UserSummary for User {
    @required
    $userId

    @required
    $firstName

    @required
    $lastName
}

list Users {
    member: UserSummary
}

@readonly
@http(method: "GET", uri: "/users/{userId}")
operation GetUser {
    input := for User {
        @required
        @httpLabel
        $userId
    }

    output := for User {
        @required
        $userId

        @required
        $firstName

        @required
        $lastName

        $timezone
    }

    errors: [
        PeoplesRepublicOfListsError
    ]
}

@idempotent
@http(method: "PUT", uri: "/users/{userId}")
operation UpdateUser {
    input := for User {
        @required
        @httpLabel
        $userId

        @required
        $firstName

        @required
        $lastName

        /// IANA timezone name (e.g. "America/New_York"). Optional — omitting it leaves
        /// the user's current timezone unchanged, unlike firstName/lastName which are
        /// always overwritten. Used by the Google Calendar import sync to resolve
        /// all-day event dates into the correct UTC instant for this user.
        $timezone
    }

    output := {}

    errors: [
        PeoplesRepublicOfListsError
    ]
}

@http(method: "POST", uri: "/users/{userId}/app-invites")
operation SendAppInvite {
    input := {
        @required
        @httpLabel
        userId: String

        @required
        @notProperty
        email: String
    }

    output := {}

    errors: [
        PeoplesRepublicOfListsError
    ]
}

@readonly
@http(method: "GET", uri: "/users")
operation ListUsers {
    input := {}

    output := {
        @required
        users: Users
    }

    errors: [
        PeoplesRepublicOfListsError
    ]
}
