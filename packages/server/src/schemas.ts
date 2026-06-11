import { z } from '@hono/zod-openapi'

// ─── Enums ──────────────────────────────────────────────────────────────────

export const TaskStatusSchema = z.enum([
  'pending',
  'assigned',
  'running',
  'paused',
  'blocked',
  'completed',
  'failed',
  'timeout',
  'cancelled',
])

export const LevelSchema = z.enum(['debug', 'info', 'warn', 'error'])

export const SeriesModeSchema = z.enum(['keep-all', 'accumulate', 'latest'])

export const AssignModeSchema = z.enum(['external', 'pull', 'ws-offer', 'ws-race'])

export const DisconnectPolicySchema = z.enum(['reassign', 'mark', 'fail'])

// ─── Response Schemas ───────────────────────────────────────────────────────

export const TaskErrorSchema = z
  .object({
    code: z.string().optional(),
    message: z.string(),
    details: z.record(z.unknown()).optional(),
  })
  .openapi('TaskError')

export const TaskSchema = z
  .object({
    id: z.string(),
    type: z.string().optional(),
    status: TaskStatusSchema,
    params: z.record(z.unknown()).optional(),
    result: z.record(z.unknown()).optional(),
    error: TaskErrorSchema.optional(),
    metadata: z.record(z.unknown()).optional(),
    createdAt: z.number(),
    updatedAt: z.number(),
    completedAt: z.number().optional(),
    ttl: z.number().int().positive().optional(),
    tags: z.array(z.string()).optional(),
    assignMode: AssignModeSchema.optional(),
    cost: z.number().int().positive().optional(),
    assignedWorker: z.string().optional(),
    disconnectPolicy: DisconnectPolicySchema.optional(),
  })
  .openapi('Task')

export const TaskEventSchema = z
  .object({
    id: z.string(),
    taskId: z.string(),
    index: z.number().int(),
    timestamp: z.number(),
    type: z.string(),
    level: LevelSchema,
    data: z.unknown(),
    seriesId: z.string().optional(),
    seriesMode: SeriesModeSchema.optional(),
    seriesAccField: z.string().optional(),
    clientId: z.string().optional(),
    clientSeq: z.number().int().optional(),
  })
  .openapi('TaskEvent')

export const WorkerSchema = z
  .object({
    id: z.string(),
    status: z.enum(['idle', 'busy', 'draining', 'offline']),
    matchRule: z.object({
      taskTypes: z.array(z.string()).optional(),
      tags: z
        .object({
          all: z.array(z.string()).optional(),
          any: z.array(z.string()).optional(),
          none: z.array(z.string()).optional(),
        })
        .optional(),
    }),
    capacity: z.number().int(),
    usedSlots: z.number().int(),
    weight: z.number(),
    connectionMode: z.enum(['pull', 'websocket']),
    connectedAt: z.number(),
    lastHeartbeatAt: z.number(),
    metadata: z.record(z.unknown()).optional(),
  })
  .openapi('Worker')

export const ErrorSchema = z
  .object({
    error: z.string(),
  })
  .openapi('Error')

// ─── Request Body Schemas ───────────────────────────────────────────────────

export const CreateTaskSchema = z
  .object({
    id: z.string().optional(),
    type: z.string().optional(),
    params: z.record(z.unknown()).optional(),
    metadata: z.record(z.unknown()).optional(),
    ttl: z.number().int().positive().optional(),
    webhooks: z.array(z.unknown()).optional(),
    cleanup: z.object({ rules: z.array(z.unknown()) }).optional(),
    tags: z.array(z.string()).optional(),
    assignMode: AssignModeSchema.optional(),
    cost: z.number().int().positive().optional(),
    disconnectPolicy: DisconnectPolicySchema.optional(),
    authConfig: z.record(z.unknown()).optional(),
  })
  .openapi('CreateTaskInput')

export const TransitionSchema = z
  .object({
    status: TaskStatusSchema,
    result: z.record(z.unknown()).optional(),
    error: z
      .object({
        code: z.string().optional(),
        message: z.string(),
        details: z.record(z.unknown()).optional(),
      })
      .optional(),
    reason: z.string().optional(),
    ttl: z.number().int().positive().optional(),
    resumeAfterMs: z.number().int().positive().optional(),
    blockedRequest: z.object({ type: z.string(), data: z.unknown() }).optional(),
  })
  .openapi('TransitionInput')

export const SeqModeSchema = z.enum(['hold', 'fast-fail'])

export const PublishEventSchema = z
  .object({
    type: z.string(),
    level: LevelSchema,
    data: z.unknown(),
    seriesId: z.string().optional(),
    seriesMode: SeriesModeSchema.optional(),
    seriesAccField: z.string().optional(),
    clientId: z.string().optional(),
    clientSeq: z.number().int().min(0).optional(),
    seqMode: SeqModeSchema.optional(),
  })
  .refine(
    (d) => (d.clientId === undefined) === (d.clientSeq === undefined),
    { message: 'clientId and clientSeq must both be present or both be absent' },
  )
  .openapi('PublishEventInput')

export const DeclineSchema = z
  .object({
    workerId: z.string(),
    blacklist: z.boolean().optional(),
  })
  .openapi('DeclineInput')

export const WorkerStatusUpdateSchema = z
  .object({
    status: z.enum(['draining', 'idle']),
  })
  .openapi('WorkerStatusUpdateInput')
