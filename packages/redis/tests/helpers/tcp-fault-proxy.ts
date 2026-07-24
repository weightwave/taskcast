import {
  createConnection,
  createServer,
  type Server,
  type Socket,
} from 'node:net'

export type RequestMatcher = (request: Buffer) => boolean

type ProxyMode = 'open' | 'refuse' | 'blackhole'

interface ProxyConnection {
  downstream: Socket
  upstream?: Socket
  requestBuffer: Buffer
  dropResponse: boolean
}

interface ResponseDropRule {
  matcher: RequestMatcher
  eligibleConnections: Set<ProxyConnection>
}

const MAX_MATCHER_BUFFER_BYTES = 64 * 1024

export function redisCommandMatcher(
  command: string,
  ...arguments_: string[]
): RequestMatcher {
  const expected = [command, ...arguments_].map((part) => Buffer.from(part))
  return (request) => firstRespCommandMatches(request, expected)
}

function firstRespCommandMatches(request: Buffer, expected: Buffer[]): boolean {
  const parsed = parseRespCommand(request, 0)
  return parsed !== undefined
    && parsed.parts.length === expected.length
    && parsed.parts.every((part, index) =>
      index === 0
        ? part.toString('ascii').toUpperCase()
          === expected[index]!.toString('ascii').toUpperCase()
        : part.equals(expected[index]!),
    )
}

function parseRespCommand(
  request: Buffer,
  offset: number,
): { parts: Buffer[], end: number } | undefined {
  if (request[offset] !== 0x2a) return undefined
  const arrayLine = readRespLine(request, offset + 1)
  if (arrayLine === undefined) return undefined
  const partCount = Number.parseInt(arrayLine.value.toString('ascii'), 10)
  if (!Number.isSafeInteger(partCount) || partCount < 0) return undefined

  const parts: Buffer[] = []
  let cursor = arrayLine.end
  for (let index = 0; index < partCount; index++) {
    if (request[cursor] !== 0x24) return undefined
    const lengthLine = readRespLine(request, cursor + 1)
    if (lengthLine === undefined) return undefined
    const length = Number.parseInt(lengthLine.value.toString('ascii'), 10)
    if (!Number.isSafeInteger(length) || length < 0) return undefined
    const end = lengthLine.end + length
    if (
      end + 2 > request.length
      || request[end] !== 0x0d
      || request[end + 1] !== 0x0a
    ) {
      return undefined
    }
    parts.push(request.subarray(lengthLine.end, end))
    cursor = end + 2
  }
  return { parts, end: cursor }
}

function readRespLine(
  request: Buffer,
  offset: number,
): { value: Buffer, end: number } | undefined {
  const lineEnd = request.indexOf('\r\n', offset)
  if (lineEnd === -1) return undefined
  return {
    value: request.subarray(offset, lineEnd),
    end: lineEnd + 2,
  }
}

export class TcpFaultProxy {
  private server: Server | undefined
  private readonly sockets = new Set<Socket>()
  private readonly connections: ProxyConnection[] = []
  private listenPort = 0
  private accepted = 0
  private matched = 0
  private mode: ProxyMode = 'open'
  private responseDropRule: ResponseDropRule | undefined

  constructor(
    private readonly upstreamHost: string,
    private readonly upstreamPort: number,
  ) {}

  get port(): number {
    if (this.listenPort === 0) throw new Error('proxy has not been opened')
    return this.listenPort
  }

  get acceptedConnections(): number {
    return this.accepted
  }

  get matchedCommands(): number {
    return this.matched
  }

  async open(): Promise<void> {
    if (this.mode === 'blackhole') this.closeSockets()
    this.mode = 'open'
    if (this.server?.listening) return

    const server = createServer((downstream) => {
      this.accepted++
      this.track(downstream)
      if (this.mode === 'refuse') {
        downstream.resetAndDestroy()
        return
      }

      const connection: ProxyConnection = {
        downstream,
        requestBuffer: Buffer.alloc(0),
        dropResponse: false,
      }
      this.connections.push(connection)

      if (this.mode === 'blackhole') {
        downstream.on('data', () => {
          // Deliberately consume without forwarding.
        })
        return
      }

      const upstream = createConnection(this.upstreamPort, this.upstreamHost)
      connection.upstream = upstream
      this.track(upstream)

      downstream.on('data', (chunk: Buffer) => {
        if (this.mode !== 'open' || upstream.destroyed) return
        this.inspectRequest(connection, chunk)
        upstream.write(chunk)
      })
      upstream.on('data', (chunk: Buffer) => {
        if (connection.dropResponse) {
          downstream.destroy()
          upstream.destroy()
          return
        }
        if (this.mode === 'open' && !downstream.destroyed) {
          downstream.write(chunk)
        }
      })
      downstream.once('close', () => upstream.destroy())
      upstream.once('close', () => downstream.destroy())
    })
    this.server = server
    await this.listen(server, this.listenPort)
  }

  async blackhole(): Promise<void> {
    this.mode = 'blackhole'
  }

  async refuse(): Promise<void> {
    this.mode = 'refuse'
    this.closeSockets()
    await this.closeServer()
  }

  pauseNewConnections(): void {
    this.mode = 'refuse'
  }

  resumeNewConnections(): void {
    this.mode = 'open'
  }

  dropNextResponse(matcher: RequestMatcher): void {
    if (this.responseDropRule !== undefined) {
      throw new Error('a response-drop matcher is already armed')
    }
    const eligibleConnections = new Set(
      this.connections.filter(({ downstream, upstream }) =>
        !downstream.destroyed
        && upstream !== undefined
        && !upstream.destroyed,
      ),
    )
    if (eligibleConnections.size === 0) {
      throw new Error('response-drop matcher requires an established connection')
    }
    this.responseDropRule = { matcher, eligibleConnections }
    for (const connection of eligibleConnections) {
      connection.requestBuffer = Buffer.alloc(0)
    }
  }

  closeLatestConnection(): void {
    const connection = this.connections
      .slice()
      .reverse()
      .find(({ downstream, upstream }) =>
        !downstream.destroyed || (upstream !== undefined && !upstream.destroyed),
      )
    connection?.downstream.destroy()
    connection?.upstream?.destroy()
  }

  closeSockets(): void {
    for (const socket of this.sockets) socket.resetAndDestroy()
    this.sockets.clear()
  }

  async stop(): Promise<void> {
    this.mode = 'refuse'
    this.responseDropRule = undefined
    this.closeSockets()
    await this.closeServer()
  }

  private inspectRequest(connection: ProxyConnection, chunk: Buffer): void {
    const rule = this.responseDropRule
    if (rule === undefined || !rule.eligibleConnections.has(connection)) return

    connection.requestBuffer = Buffer.concat([
      connection.requestBuffer,
      chunk,
    ])
    if (connection.requestBuffer.length > MAX_MATCHER_BUFFER_BYTES) {
      connection.requestBuffer = connection.requestBuffer.subarray(
        connection.requestBuffer.length - MAX_MATCHER_BUFFER_BYTES,
      )
    }
    if (!rule.matcher(connection.requestBuffer)) return

    connection.requestBuffer = Buffer.alloc(0)
    this.matched++
    connection.dropResponse = true
    this.responseDropRule = undefined
    for (const trackedConnection of this.connections) {
      trackedConnection.requestBuffer = Buffer.alloc(0)
    }
  }

  private track(socket: Socket): void {
    this.sockets.add(socket)
    socket.once('close', () => this.sockets.delete(socket))
    socket.once('error', () => socket.destroy())
  }

  private async listen(server: Server, port: number): Promise<void> {
    await new Promise<void>((resolve, reject) => {
      const onError = (error: Error) => {
        server.off('listening', onListening)
        reject(error)
      }
      const onListening = () => {
        server.off('error', onError)
        const address = server.address()
        if (!address || typeof address === 'string') {
          reject(new Error('proxy did not receive a TCP address'))
          return
        }
        this.listenPort = address.port
        resolve()
      }
      server.once('error', onError)
      server.once('listening', onListening)
      server.listen(port, '127.0.0.1')
    })
  }

  private async closeServer(): Promise<void> {
    const server = this.server
    this.server = undefined
    if (!server?.listening) return
    await new Promise<void>((resolve, reject) => {
      server.close((error) => (error ? reject(error) : resolve()))
    })
  }
}
