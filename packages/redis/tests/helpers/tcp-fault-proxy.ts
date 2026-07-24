import {
  createConnection,
  createServer,
  type Server,
  type Socket,
} from 'node:net'

export class TcpFaultProxy {
  private server: Server | undefined
  private readonly sockets = new Set<Socket>()
  private listenPort = 0
  private accepted = 0

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

  async open(): Promise<void> {
    if (this.server?.listening) return
    const server = createServer((downstream) => {
      this.accepted++
      const upstream = createConnection(this.upstreamPort, this.upstreamHost)
      this.track(downstream)
      this.track(upstream)
      downstream.pipe(upstream)
      upstream.pipe(downstream)
    })
    this.server = server

    let candidate = this.listenPort || 20_000 + (process.pid % 20_000)
    for (;;) {
      try {
        await this.listen(server, candidate)
        return
      } catch (error) {
        if (
          this.listenPort === 0 &&
          (error as NodeJS.ErrnoException).code === 'EADDRINUSE' &&
          candidate < 40_000
        ) {
          candidate++
          continue
        }
        throw error
      }
    }
  }

  async refuse(): Promise<void> {
    this.closeSockets()
    await this.closeServer()
  }

  closeSockets(): void {
    for (const socket of this.sockets) socket.destroy()
    this.sockets.clear()
  }

  async stop(): Promise<void> {
    await this.refuse()
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
