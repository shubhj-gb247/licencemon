import asyncio
import httpx
import random
import time
import socket

URL = "http://localhost:8000/update_usage"
NUM_REQUESTS = 10000   # total requests to send
CONCURRENCY = 100     # number of requests in parallel

# get hostname for payload
hostname = socket.gethostname()

def make_payload():
    # simulate random app usage
    apps = ["chrome.exe", "notepad.exe", "brave.exe", "teams.exe"]
    app = random.choice(apps)
    duration = random.randint(1, 10)  # seconds
    return {
        "hostname": hostname,
        app: {"duration": duration}
    }

async def worker(client, task_id, results):
    payload = make_payload()
    try:
        resp = await client.post(URL, json=payload)
        results.append(resp.status_code)
    except Exception as e:
        print(f"Task {task_id} failed: {e}")
        results.append("fail")

async def run_load_test():
    results = []
    async with httpx.AsyncClient(timeout=10.0) as client:
        tasks = []
        start = time.perf_counter()

        for i in range(NUM_REQUESTS):
            task = worker(client, i, results)
            tasks.append(task)

            # dispatch in batches of CONCURRENCY
            if len(tasks) >= CONCURRENCY:
                await asyncio.gather(*tasks)
                tasks.clear()

        # flush remaining
        if tasks:
            await asyncio.gather(*tasks)

        end = time.perf_counter()
        duration = end - start
        success = results.count(200)
        failed = len(results) - success

        print(f"Sent {NUM_REQUESTS} requests in {duration:.2f} seconds")
        print(f"Throughput: {NUM_REQUESTS/duration:.2f} req/s")
        print(f"Success: {success}, Failed: {failed}")

if __name__ == "__main__":
    asyncio.run(run_load_test())
