#!/usr/bin/env node
import { Command } from 'commander'
import { registerStartCommand } from './commands/start.js'
import { registerMigrateCommand } from './commands/migrate.js'
import { registerPlaygroundCommand } from './commands/playground.js'
import { registerUiCommand } from './commands/ui.js'

const program = new Command()

program
  .name('taskcast')
  .description('Taskcast — unified task tracking and streaming service')
  .version('0.3.1')

registerStartCommand(program)
registerMigrateCommand(program)
registerPlaygroundCommand(program)
registerUiCommand(program)

// Placeholders for unimplemented commands
program.command('daemon').description('Start as background service (not yet implemented)')
  .action(() => { console.error('[taskcast] daemon mode is not yet implemented'); process.exit(1) })
program.command('stop').description('Stop background service (not yet implemented)')
  .action(() => { console.error('[taskcast] stop is not yet implemented'); process.exit(1) })
program.command('status').description('Show server status (not yet implemented)')
  .action(() => { console.error('[taskcast] status is not yet implemented'); process.exit(1) })

program.parse()
