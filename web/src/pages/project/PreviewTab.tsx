import type { Project } from "../../lib/types";

export default function PreviewTab({ project }: { project: Project }) {
  return <div className="text-sm text-muted">Live preview for {project.name} is coming in the next step.</div>;
}
