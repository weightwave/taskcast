import {
  Tabs,
  TabsContent,
  TabsList,
  TabsTrigger,
} from '@/components/ui/tabs'
import { TaskList } from '@/components/bottom/TaskList'
import { EventHistory } from '@/components/bottom/EventHistory'
import { WebhookLogs } from '@/components/bottom/WebhookLogs'

export function BottomArea() {
  return (
    <div className="h-[250px] border-t">
      <Tabs defaultValue="tasks" className="h-full">
        <TabsList className="mx-2 mt-1">
          <TabsTrigger value="tasks">Tasks</TabsTrigger>
          <TabsTrigger value="events">Event History</TabsTrigger>
          <TabsTrigger value="webhooks">Webhook Logs</TabsTrigger>
        </TabsList>

        <TabsContent value="tasks" className="px-4 py-2 h-[calc(100%-44px)]">
          <TaskList />
        </TabsContent>

        <TabsContent value="events" className="px-4 py-2 h-[calc(100%-44px)]">
          <EventHistory />
        </TabsContent>

        <TabsContent value="webhooks" className="px-4 py-2 h-[calc(100%-44px)]">
          <WebhookLogs />
        </TabsContent>
      </Tabs>
    </div>
  )
}
