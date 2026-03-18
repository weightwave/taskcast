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

// Placeholders for unimplemented commands
program.command('daemon').description('Start as background service (not yet implemented)')
  .action(() => { console.error('[taskcast] daemon mode is not yet implemented'); process.exit(1) })
program.command('stop').description('Stop background service (not yet implemented)')
  .action(() => { console.error('[taskcast] stop is not yet implemented'); process.exit(1) })
program.command('status').description('Show server status (not yet implemented)')
  .action(() => { console.error('[taskcast] status is not yet implemented'); process.exit(1) })

program.parse()
