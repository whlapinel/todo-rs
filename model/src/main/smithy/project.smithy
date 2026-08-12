$version: "2"

namespace common

// Project is deliberately NOT a Smithy `resource`, mirroring Team's own precedent
// (see team.smithy) — it's a plain set of operations, some scoped under User
// (CreateProject/GetProject/UpdateProject/DeleteProject/ListProjects, added to
// User's own `operations: [...]` list in user.smithy) and some scoped directly
// under `/projects/{projectId}` with no userId prefix (ListProjectMembers/
// SetProjectMemberRole/AttachTeamToProject/DetachTeamFromProject, added to
// service.smithy's top-level `operations: [...]` list — same placement as
// TeamItem's own CRUD operations).
//
// AttachTeamToProject/DetachTeamFromProject exist instead of folding team
// attach/switch/detach into UpdateProject's teamId field, because this API has no
// existing convention for distinguishing "field omitted" from "field explicitly
// cleared" — every other optional field in this model (dueDate, assignedToUserId,
// parentItemId, etc.) is a plain direct-overwrite Option, with "preserve the
// current value" handled by the caller round-tripping it, not by the server
// detecting JSON null vs. absent. Two dedicated operations avoid inventing that
// convention for this one field, and map 1:1 onto service::projects's
// attach_team_to_project/detach_team_from_project (stage A4).
structure ProjectSummary {
    @required
    projectId: String

    @required
    name: String

    @required
    ownerUserId: String

    teamId: String
}

list Projects {
    member: ProjectSummary
}

structure ProjectMemberSummary {
    @required
    userId: String

    @required
    firstName: String

    @required
    lastName: String

    @required
    role: String

    @required
    points: Integer
}

list ProjectMembers {
    member: ProjectMemberSummary
}

@http(method: "POST", uri: "/users/{userId}/projects")
operation CreateProject {
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
        projectId: String
    }

    errors: [
        PeoplesRepublicOfListsError
    ]
}

@readonly
@http(method: "GET", uri: "/users/{userId}/projects/{projectId}")
operation GetProject {
    input := {
        @required
        @httpLabel
        userId: String

        @required
        @httpLabel
        @notProperty
        projectId: String
    }

    output := {
        @required
        @notProperty
        projectId: String

        @required
        @notProperty
        name: String

        @required
        @notProperty
        ownerUserId: String

        @notProperty
        teamId: String
    }

    errors: [
        PeoplesRepublicOfListsError
    ]
}

@idempotent
@http(method: "PUT", uri: "/users/{userId}/projects/{projectId}")
operation UpdateProject {
    input := {
        @required
        @httpLabel
        userId: String

        @required
        @httpLabel
        @notProperty
        projectId: String

        @required
        @notProperty
        name: String
    }

    output := {}

    errors: [
        PeoplesRepublicOfListsError
    ]
}

@idempotent
@http(method: "DELETE", uri: "/users/{userId}/projects/{projectId}")
operation DeleteProject {
    input := {
        @required
        @httpLabel
        userId: String

        @required
        @httpLabel
        @notProperty
        projectId: String
    }

    output := {}

    errors: [
        PeoplesRepublicOfListsError
    ]
}

@readonly
@http(method: "GET", uri: "/users/{userId}/projects")
operation ListProjects {
    input := {
        @required
        @httpLabel
        userId: String
    }

    output := {
        @required
        @notProperty
        projects: Projects
    }

    errors: [
        PeoplesRepublicOfListsError
    ]
}

@readonly
@http(method: "GET", uri: "/projects/{projectId}/members")
operation ListProjectMembers {
    input := {
        @required
        @httpLabel
        projectId: String
    }

    output := {
        @required
        members: ProjectMembers
    }

    errors: [
        PeoplesRepublicOfListsError
    ]
}

@idempotent
@http(method: "PUT", uri: "/projects/{projectId}/members/{targetUserId}/role")
operation SetProjectMemberRole {
    input := {
        @required
        @httpLabel
        projectId: String

        @required
        @httpLabel
        targetUserId: String

        @required
        role: String
    }

    output := {}

    errors: [
        PeoplesRepublicOfListsError
    ]
}

@idempotent
@http(method: "PUT", uri: "/projects/{projectId}/team/{teamId}")
operation AttachTeamToProject {
    input := {
        @required
        @httpLabel
        projectId: String

        @required
        @httpLabel
        teamId: String
    }

    output := {}

    errors: [
        PeoplesRepublicOfListsError
    ]
}

@idempotent
@http(method: "DELETE", uri: "/projects/{projectId}/team")
operation DetachTeamFromProject {
    input := {
        @required
        @httpLabel
        projectId: String
    }

    output := {}

    errors: [
        PeoplesRepublicOfListsError
    ]
}
