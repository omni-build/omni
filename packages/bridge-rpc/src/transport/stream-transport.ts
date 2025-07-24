import { AbstractTransport } from "./abstract-transport";
import type { Transport } from "./interface";

export type StreamTransportConfig = {
    input: ReadableStream<Uint8Array>;
    output: WritableStream<Uint8Array>;
};

export class StreamTransport extends AbstractTransport implements Transport {
    private writer: WritableStreamDefaultWriter<Uint8Array>;
    constructor(config: StreamTransportConfig) {
        super();
        this.writer = config.output.getWriter();
        config.input.pipeTo(
            new WritableStream({
                write: this.receiveBytes,
            }),
        );
    }

    protected override async sendBytes(data: Uint8Array): Promise<void> {
        await this.writer.write(data);
    }
}
