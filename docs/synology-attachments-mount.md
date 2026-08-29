# Mounting a Synology share for attachments

Directions for pointing this app's file-attachment storage (`TODO_ATTACHMENTS_DIR`, see
root CLAUDE.md's Attachments section) at a Synology NAS instead of local disk on the box
running the server. The app itself doesn't know or care whether `TODO_ATTACHMENTS_DIR`
is local disk or a network mount — this doc is entirely about the OS-level mount, not
anything app-specific.

## 1. On the Synology (DSM)

1. **Create a dedicated shared folder** — Control Panel → Shared Folder → Create, e.g.
   `todo-attachments`. Don't reuse an existing personal folder; keep the blast radius
   contained to this one purpose.
2. **Enable SMB** — Control Panel → File Services → SMB. Enable SMB2/3 and disable SMB1
   if it's on. (NFS is a lighter-weight alternative if you'd rather use it, but SMB has
   broader auth/permission tooling on DSM and is the more common default.)
3. **Create a dedicated service account** — Control Panel → User & Group → Create, e.g.
   `todo-svc`. Give it a strong password.
4. **Scope that account's permissions to only the new shared folder** — Control Panel →
   Shared Folder → `todo-attachments` → Edit → Permissions tab → grant `todo-svc`
   Read/Write, and make sure it has no access to any other shared folder. If these
   credentials ever leak from the app server, they should only be able to touch this one
   folder.
5. **Give the NAS a stable address** — set a DHCP reservation for it on your router (or a
   static IP on the NAS itself), so the mount below doesn't silently break after a router
   reboot reassigns its lease.

## 2. On the box running the server

1. **Install CIFS mount support**, if not already present:

   ```sh
   sudo apt install cifs-utils   # Debian/Ubuntu
   ```

2. **Create the mount point and a credentials file:**

   ```sh
   sudo mkdir -p /mnt/todo-attachments
   sudo tee /etc/todo-attachments-creds >/dev/null <<'EOF'
   username=todo-svc
   password=REPLACE_ME
   EOF
   sudo chmod 600 /etc/todo-attachments-creds
   ```

   The credentials file must be `chmod 600` and owned by root — it holds a plaintext
   password.

3. **Add an `/etc/fstab` entry** (replace `NAS_HOST_OR_IP` and the app's own uid/gid —
   run `id todo` or whatever user runs the server process to find them):

   ```
   //NAS_HOST_OR_IP/todo-attachments  /mnt/todo-attachments  cifs  credentials=/etc/todo-attachments-creds,uid=1000,gid=1000,iocharset=utf8,vers=3.0,_netdev  0  0
   ```

   - `_netdev` tells the OS to wait for networking before attempting this mount at boot
     — without it, a mount attempted too early can silently fail.
   - `uid`/`gid` should match the user the `todo` server process runs as, so it can
     actually read/write the files without everything ending up root-owned.
   - `vers=3.0` pins SMB3; drop it (or set `vers=2.1`) if the Synology is older/configured
     for SMB2 only.

4. **Mount it and verify:**

   ```sh
   sudo mount -a
   mount | grep todo-attachments
   touch /mnt/todo-attachments/write-test && rm /mnt/todo-attachments/write-test
   ```

   The `touch`/`rm` confirms the app's own user can actually write to the share, not just
   that root mounted it.

## 3. Point the app at it

Set the env var the server reads at startup:

```sh
TODO_ATTACHMENTS_DIR=/mnt/todo-attachments
```

Restart the server. No other configuration is needed — the local-filesystem attachment
store (`storage::attachment_store::LocalFsAttachmentStore`) just reads/writes files under
whatever directory this points at, mount or not.

## Failure mode to know about

If the NAS goes offline or the mount drops, attachment upload/download requests will
fail (the app surfaces this as a normal error, not a crash) — everything else in the app
keeps working normally, since attachments are the only thing that touches this directory.
If that's ever a problem in practice, `mount -a` after the NAS comes back is enough to
restore it; the app doesn't need a restart.
