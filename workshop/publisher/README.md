# PERPHEADS Texture Helper Workshop

Community skin storage and automated validation for
[Texture Helper](https://github.com/JoeyMarrey29/TextureHelper).
Public uploads require Steam sign-in and pass automated validation before publication.

Use the desktop application's Skin Workshop to submit and download skins.
Uploads require Steam sign-in and explicit permission to share. Submissions
remain unpublished draft releases until automated validation succeeds. They are
data, never executable code or workflow inputs to a shell.

Published skin packages and previews live in GitHub Releases, not the Git
history. Downloads become local editable drafts; the app never installs them
automatically. Code here is GPL-3.0; uploader-owned skin contributions use
CC BY 4.0 with Steam attribution. No game models or original game textures
are distributed in this repository.

The workflow uses only a standard GitHub-hosted runner in this public repository.
Do not switch to a paid/larger runner, make this repository private or give
untrusted contributors access to Actions secrets. No pull-request workflow
should receive publishing credentials.

Service setup, security boundaries and limits are documented in the
[application repository](https://github.com/JoeyMarrey29/TextureHelper/tree/main/workshop).
