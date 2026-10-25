import { useQuery } from "@tanstack/react-query";
import { Badge, Card, PageHeader } from "../../components/ui";
import { api } from "../../lib/api";
import { dateTime } from "../../lib/format";

export default function Events() {
  const q = useQuery({ queryKey: ["events"], queryFn: () => api.get<any[]>("/api/events"), refetchInterval: 30_000 });
  return (
    <>
      <PageHeader title="Activity log" subtitle="Updates, warnings and errors from the panel" />
      <Card className="divide-y divide-border">
        {q.data?.length === 0 && <div className="p-6 text-sm text-muted text-center">Nothing yet</div>}
        {q.data?.map((e) => (
          <div key={e.id} className="px-4 py-2.5 flex gap-3 text-sm">
            <Badge tone={e.level === "error" ? "danger" : e.level === "warn" ? "warn" : "neutral"} className="shrink-0 h-fit">
              {e.level}
            </Badge>
            <div className="min-w-0">
              <div className="whitespace-pre-wrap break-words">{e.message}</div>
              <div className="text-xs text-muted">{dateTime(e.created_at)}</div>
            </div>
          </div>
        ))}
      </Card>
    </>
  );
}
