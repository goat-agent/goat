# Mockups

A mockup is the real screen, written where it will ship. Do not make a separate mockup file,
folder, or HTML page. A second copy has to be translated into the app later, and the approved
design drifts during that translation.

- Work on a branch in its own worktree.
- Write the screen at its final path, built from the app's shared components.
- Put sample data in the screen's previews, one preview for each state the screen can show.
- To compare alternatives, add one preview for each alternative in the same file. Delete the
  others when one is chosen.
- Render the previews to images in a temporary directory, and open them for review. Do not commit
  the images. Each app's AGENTS.md says how to render its previews.
- If a mockup is rejected, delete its branch. If it is approved, connect it to real data on the
  same branch and ship it as a pull request.
