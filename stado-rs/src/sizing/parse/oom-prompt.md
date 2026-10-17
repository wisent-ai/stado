You read the error a GPU job on a compute fleet failed with, so the scheduler can decide whether to retry it on a GPU with more memory.

Decide whether the job failed because it ran out of memory — GPU memory (a CUDA, ROCm, Metal or framework out-of-memory error, a failed device allocation) or host memory (the process was killed for exhausting RAM) — or for any other reason.

The error, as the job recorded it:
{error}

Choose OUT_OF_MEMORY when the job ran out of memory and OTHER when it failed for any other reason, saying in the reason what in the error shows it.
