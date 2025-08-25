from fastapi import FastAPI, HTTPException,Request
from pydantic import BaseModel, Extra
from typing import Dict, Any
import asyncpg
import uvicorn

# -------------------------------
# Pydantic model for incoming payload
# -------------------------------
class AppUsage(BaseModel):
    hostname: str
    apps: Dict[str, Any]  # Each app can be a dict with extra fields

    class Config:
        extra = Extra.allow  # Accept extra top-level keys

# -------------------------------
# FastAPI app
# -------------------------------
app = FastAPI()

# -------------------------------
# PostgreSQL connection pool
# -------------------------------
DB_USER = "postgres"
DB_PASSWORD = "postgres"
DB_HOST = "localhost"
DB_PORT = 5432
DB_NAME = "licensemon"

pool: asyncpg.pool.Pool = None

@app.on_event("startup")
async def startup():
    global pool
    pool = await asyncpg.create_pool(
        user=DB_USER,
        password=DB_PASSWORD,
        database=DB_NAME,
        host=DB_HOST,
        port=DB_PORT,
        min_size=1,
        max_size=50,
    )

@app.on_event("shutdown")
async def shutdown():
    await pool.close()

# -------------------------------
# Endpoint to receive data
# -------------------------------
@app.post("/update_usage")
async def update_usage(request: Request):
    payload = await request.json()
    print(type(payload))
    # Extract hostname
    hostname = payload["hostname"]
    if not hostname:
        raise HTTPException(status_code=400, detail="Missing 'hostname' field")

    async with pool.acquire() as conn:
        async with conn.transaction():
            for key, value in payload.items():
                if key == "hostname":
                    continue
                if isinstance(value, dict):
                    duration = value.get("duration", 0)
                    await conn.execute(
                        """
                        INSERT INTO licensemonitor_table(hostname, app_name, duration_s, timestamp)
                        VALUES($1, $2, $3, now())
                        ON CONFLICT(hostname, app_name)
                        DO UPDATE SET
                            duration_s = licensemonitor_table.duration_s + EXCLUDED.duration_s,
                            timestamp = now(); 
                        """,
                        hostname,
                        key,
                        duration
                    )

    return {"status": "ok"}

# -------------------------------
# Run server
# -------------------------------
if __name__ == "__main__":
    uvicorn.run(app, host="0.0.0.0", port=8000)
