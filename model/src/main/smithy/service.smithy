$version: "2"

namespace common

use aws.protocols#restJson1

@restJson1
@httpBearerAuth
service PeoplesRepublicOfLists {
    version: "2026-04-14"
    resources: [
        User
    ]
    operations: [
        CreateTeamItem
        GetTeamItem
        UpdateTeamItem
        DeleteTeamItem
        ListTeamItems
        CreateTeamTemplate
        ListTeamTemplates
        ListTeamActivityLog
        UndoActivityLogEntry
        ListProjectMembers
        SetProjectMemberRole
        AttachTeamToProject
        DetachTeamFromProject
    ]
}
