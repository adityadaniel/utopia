import Foundation

@MainActor
final class LineWork {
    private var tasks: [UUID: Task<Void, Never>] = [:]
    private var generation = 0

    func enqueue(for id: UUID, _ work: @escaping @MainActor () async -> Void) {
        let previous = tasks[id]
        generation += 1
        tasks[id] = Task { @MainActor in
            _ = await previous?.value
            await work()
        }
    }

    func waitForAll() async {
        while true {
            let seen = generation
            for task in Array(tasks.values) {
                await task.value
            }
            if seen == generation { return }
        }
    }

    func remove(_ id: UUID) {
        tasks[id] = nil
    }

    func removeAll() {
        tasks.removeAll()
    }
}
