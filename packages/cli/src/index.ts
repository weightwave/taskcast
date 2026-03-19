#!/usr/bin/env node
import { Command } from 'commander'
import { registerStartCommand } from './commands/start.js'
import { registerMigrateCommand } from './commands/migrate.js'
import { registerPlaygroundCommand } from './commands/playground.js'
import { registerUiCommand } from './commands/ui.js'
import { registerNodeCommand } from './commands/node.js'
import { registerPingCommand } from './commands/ping.js'
import { registerDoctorCommand } from './commands/doctor.js'
import { registerLogsCommand, registerTailCommand } from './commands/logs.js'
import { registerTasksCommand } from './commands/tasks.js'
import { registerServiceCommand } from './commands/service.js'

const program = new Command()

program
  .name('taskcast')
  .description('Taskcast — unified task tracking and streaming service')
  .version('0.3.1')

registerStartCommand(program)
registerMigrateCommand(program)
registerPlaygroundCommand(program)
registerUiCommand(program)
registerNodeCommand(program)
registerPingCommand(program)
registerDoctorCommand(program)
registerLogsCommand(program)
registerTailCommand(program)
registerTasksCommand(program)
registerServiceCommand(program)

// Backward-compat aliases for old daemon/stop/status placeholder commands
program.command('daemon').description('Alias for `taskcast service start`')
  .action(async () => {
    const { runServiceStart } = await import('./commands/service.js')
    await runServiceStart()
  })
program.command('stop').description('Alias for `taskcast service stop`')
  .action(async () => {
    const { runServiceStop } = await import('./commands/service.js')
    await runServiceStop()
  })
program.command('status').description('Alias for `taskcast service status`')
  .action(async () => {
    const { runServiceStatus } = await import('./commands/service.js')
    await runServiceStatus()
  })

program.parse()
