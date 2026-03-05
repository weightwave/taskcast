import {
  Tabs,
  TabsContent,
  TabsList,
  TabsTrigger,
} from '@/components/ui/tabs'

export function BottomArea() {
  return (
    <div className="h-[250px] border-t">
      <Tabs defaultValue="tasks" className="h-full">
        <TabsList className="mx-2 mt-1">
          <TabsTrigger value="tasks">Tasks</TabsTrigger>
          <TabsTrigger value="events">Event History</TabsTrigger>
          <TabsTrigger value="webhooks">Webhook Logs</TabsTrigger>
        </TabsList>

        <TabsContent value="tasks" className="px-4 py-2">
          <p className="text-sm text-muted-foreground">
            Task list will appear here.
          </p>
        </TabsContent>

        <TabsContent value="events" className="px-4 py-2">
          <p className="text-sm text-muted-foreground">
            Event history will appear here.
          </p>
        </TabsContent>

        <TabsContent value="webhooks" className="px-4 py-2">
          <p className="text-sm text-muted-foreground">
            Webhook logs will appear here.
          </p>
        </TabsContent>
      </Tabs>
    </div>
  )
}
