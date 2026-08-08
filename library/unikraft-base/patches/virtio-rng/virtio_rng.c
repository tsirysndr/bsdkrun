/* SPDX-License-Identifier: BSD-3-Clause */
/* Copyright (c) 2026, The Unikraft Authors.
 * Licensed under the BSD-3-Clause License (the "License").
 * You may not use this file except in compliance with the License.
 */

/*
 * virtio-rng (VIRTIO_ID_RNG, device 4) — an entropy source backed by the host.
 *
 * The device is the simplest one in the virtio spec: a single virtqueue, no
 * configuration space, no feature bits, no request header. The guest enqueues
 * a *writable* buffer; the device fills it with entropy and returns it with
 * `len` set to however many bytes it actually produced (which may be fewer
 * than asked for).
 *
 * Why this driver exists: on arm64 the only other entropy source Unikraft has
 * is LIBUKRANDOM_LCPU, which needs FEAT_RNG (the RNDR instruction, armv8.5).
 * Guests under Hypervisor.framework do not get it, so libukrandom finds no
 * source, and the first consumer to ask for randomness — lwip's UDP init —
 * takes the whole boot down with "Could not obtain randomness (-19)". The
 * alternative, seeding from the command line, puts the CSPRNG key in the
 * kernel cmdline where it is neither secret nor unpredictable.
 *
 * Everything here is polled rather than interrupt-driven, deliberately. The
 * driver seeds libukrandom from inside its probe, which runs during the bus
 * scan before interrupts are being serviced, so waiting for a completion IRQ
 * would deadlock. virtio permits this: the used ring is shared memory, and
 * dequeue works whether or not the notification arrived.
 */

#include <string.h>

#include <uk/alloc.h>
#include <uk/assert.h>
#include <uk/essentials.h>
#include <uk/print.h>
#include <uk/sglist.h>
#include <uk/random/driver.h>
#include <virtio/virtio_bus.h>
#include <virtio/virtio_ids.h>
#include <virtio/virtqueue.h>

#define DRIVER_NAME	"virtio-rng"

/* One entry is enough: requests are issued one at a time and waited on. */
#define VTRNG_NUM_DESCS	1
#define VTRNG_HWVQ_ID	0

/* The device may return less entropy than requested, so a request loops until
 * the caller's buffer is full. Cap the number of laps so a device that keeps
 * returning zero bytes fails instead of hanging the boot.
 */
#define VTRNG_MAX_ROUNDS	16

/* Upper bound on the busy-wait for one request; see virtio_rng_read_once(). */
#define VTRNG_POLL_MAX		100000000UL

struct virtio_rng_device {
	struct virtio_dev *vdev;
	struct virtqueue *vq;
	struct uk_sglist sg;
	struct uk_sglist_seg sgsegs[1];
	/* Bounce buffer. The buffer handed to the device has to be one the
	 * device can DMA into; a caller's stack address is not guaranteed to
	 * be, and reusing one allocation avoids a per-call allocation on a
	 * path that runs before the heap is under any pressure.
	 */
	__u8 *buf;
	__sz buflen;
};

static struct uk_alloc *a;
static struct virtio_rng_device *vtrng_dev;

/* Chunk size for a single device round-trip. */
#define VTRNG_BUF_SIZE	256

/**
 * Issue one request and wait for it. Returns the number of bytes the device
 * produced (> 0), or a negative errno.
 */
static int virtio_rng_read_once(struct virtio_rng_device *d, __sz size)
{
	unsigned long spins;
	__u32 len = 0;
	void *cookie;
	int rc;

	UK_ASSERT(d);
	UK_ASSERT(size <= d->buflen);

	uk_sglist_reset(&d->sg);
	rc = uk_sglist_append(&d->sg, d->buf, size);
	if (unlikely(rc)) {
		uk_pr_err(DRIVER_NAME": Failed to append to sglist (%d)\n", rc);
		return rc;
	}

	/* 0 readable, 1 writable: the device only ever writes to us. */
	rc = virtqueue_buffer_enqueue(d->vq, d, &d->sg, 0, 1);
	if (unlikely(rc < 0)) {
		uk_pr_err(DRIVER_NAME": Failed to enqueue buffer (%d)\n", rc);
		return rc;
	}

	virtqueue_host_notify(d->vq);

	/* Poll the used ring. See the file header for why this is not an
	 * interrupt wait.
	 *
	 * Bounded, because this runs during the bus scan: a device that never
	 * completes the request would otherwise hang the boot with no output
	 * and nothing to show for it. The limit is deliberately enormous —
	 * a working device answers within a handful of iterations, so hitting
	 * it means the device is not going to answer at all.
	 */
	spins = 0;
	do {
		if (unlikely(++spins > VTRNG_POLL_MAX)) {
			uk_pr_err(DRIVER_NAME": Timed out waiting for the "
				  "device to return a buffer\n");
			return -ETIMEDOUT;
		}
		rc = virtqueue_buffer_dequeue(d->vq, &cookie, &len);
	} while (rc < 0);

	if (unlikely(len == 0)) {
		uk_pr_warn(DRIVER_NAME": Device returned no entropy\n");
		return -EAGAIN;
	}

	/* A device claiming to have written more than we offered would mean
	 * memory past the buffer was touched; refuse to trust the result.
	 */
	if (unlikely(len > size)) {
		uk_pr_err(DRIVER_NAME": Device overran the buffer (%"__PRIu32
			  " > %"__PRIsz")\n", len, size);
		return -EFAULT;
	}

	return (int)len;
}

static int virtio_rng_random_bytes(__u8 *buf, __sz size)
{
	struct virtio_rng_device *d = vtrng_dev;
	__sz filled = 0;
	unsigned int round = 0;
	int rc;

	if (unlikely(!d))
		return -ENODEV;
	if (unlikely(!buf))
		return -EINVAL;

	while (filled < size) {
		__sz want = MIN(size - filled, d->buflen);

		if (unlikely(round++ >= VTRNG_MAX_ROUNDS)) {
			uk_pr_err(DRIVER_NAME": Device did not provide %"
				  __PRIsz" bytes in %d rounds\n",
				  size, VTRNG_MAX_ROUNDS);
			return -EIO;
		}

		rc = virtio_rng_read_once(d, want);
		if (unlikely(rc < 0))
			return rc;

		memcpy(buf + filled, d->buf, (__sz)rc);
		filled += (__sz)rc;
	}

	return 0;
}

/* The host's entropy source is a DRBG seeded by the host kernel, not a raw
 * TRNG we can ask for seed-grade material separately. There is no distinct
 * seed operation to expose, so both entry points map onto the same read and
 * `seed_bytes` is the same call rather than an error — libukrandom uses it to
 * seed the CSPRNG, which is exactly what this device is for.
 */
static struct uk_random_driver_ops virtio_rng_ops = {
	.random_bytes  = virtio_rng_random_bytes,
	.seed_bytes    = virtio_rng_random_bytes,
	.seed_bytes_fb = virtio_rng_random_bytes,
};

static struct uk_random_driver virtio_rng_driver = {
	.name = DRIVER_NAME,
	.ops  = &virtio_rng_ops,
};

static int virtio_rng_vq_alloc(struct virtio_rng_device *d)
{
	__u16 qdesc_size;
	int vq_avail;

	vq_avail = virtio_find_vqs(d->vdev, 1, &qdesc_size);
	if (unlikely(vq_avail != 1)) {
		uk_pr_err(DRIVER_NAME": Expected %d queue, found %d\n",
			  1, vq_avail);
		return -ENOMEM;
	}

	uk_sglist_init(&d->sg, ARRAY_SIZE(d->sgsegs), &d->sgsegs[0]);

	/* No completion callback: this driver polls (see the file header). */
	d->vq = virtio_vqueue_setup(d->vdev, VTRNG_HWVQ_ID, qdesc_size,
				    NULL, a);
	if (unlikely(PTRISERR(d->vq))) {
		uk_pr_err(DRIVER_NAME": Failed to set up virtqueue\n");
		return PTR2ERR(d->vq);
	}

	d->vq->priv = d;

	return 0;
}

static int virtio_rng_add_dev(struct virtio_dev *vdev)
{
	struct virtio_rng_device *d;
	__u64 host_features;
	__u64 drv_features = 0;
	int rc;

	UK_ASSERT(vdev);

	/* libukrandom takes only one source of entropy, so a second device
	 * would be dead weight.
	 */
	if (unlikely(vtrng_dev)) {
		uk_pr_info(DRIVER_NAME": Ignoring additional device\n");
		return 0;
	}

	d = uk_calloc(a, 1, sizeof(*d));
	if (unlikely(!d))
		return -ENOMEM;

	d->vdev = vdev;
	d->buflen = VTRNG_BUF_SIZE;
	d->buf = uk_malloc(a, d->buflen);
	if (unlikely(!d->buf)) {
		rc = -ENOMEM;
		goto err_free_dev;
	}

	/* virtio-rng defines no feature bits of its own, but the transport's
	 * do still have to be echoed back: a modern (MMIO version 2) device
	 * is rejected outright unless the driver acknowledges
	 * VIRTIO_F_VERSION_1, which is how libkrun presents this device.
	 */
	host_features = virtio_feature_get(vdev);
	if (VIRTIO_FEATURE_HAS(host_features, VIRTIO_F_VERSION_1))
		VIRTIO_FEATURE_SET(drv_features, VIRTIO_F_VERSION_1);

	vdev->features = drv_features;
	virtio_feature_set(vdev);

	/* FEATURES_OK has to be set *before* the queues are configured. The
	 * spec orders it that way (2.4.1: set FEATURES_OK, then perform
	 * device-specific setup including virtqueue discovery), and libkrun
	 * enforces it: configuring a queue while the status is still
	 * ACK|DRIVER is refused with "update virtio queue in invalid state
	 * 0x3", and the later jump straight to 0xf is refused again as an
	 * "invalid virtio driver status transition". The device then never
	 * services the request queue and the driver waits forever.
	 */
	virtio_dev_status_update(vdev, VIRTIO_CONFIG_STATUS_ACK |
				       VIRTIO_CONFIG_STATUS_DRIVER |
				       VIRTIO_CONFIG_STATUS_FEATURES_OK);

	rc = virtio_rng_vq_alloc(d);
	if (unlikely(rc))
		goto err_status_fail;

	/* Polling, so keep the queue's interrupt masked. */
	virtqueue_intr_disable(d->vq);

	virtio_dev_drv_up(vdev);

	vtrng_dev = d;

	/* Seed libukrandom here rather than from an earlytab entry: the
	 * device does not exist until the bus scan reaches it. That scan
	 * still runs before the first consumer of randomness (lwip), so the
	 * CSPRNG is seeded in time.
	 */
	rc = uk_random_init(&virtio_rng_driver);
	if (unlikely(rc)) {
		uk_pr_err(DRIVER_NAME": Failed to register with libukrandom "
			  "(%d)\n", rc);
		vtrng_dev = NULL;
		goto err_status_fail;
	}

	uk_pr_info(DRIVER_NAME": Registered as the entropy source\n");

	return 0;

err_status_fail:
	virtio_dev_status_update(vdev, VIRTIO_CONFIG_STATUS_FAIL);
	uk_free(a, d->buf);
err_free_dev:
	uk_free(a, d);
	return rc;
}

static int virtio_rng_drv_init(struct uk_alloc *drv_allocator)
{
	if (unlikely(!drv_allocator))
		return -EINVAL;

	a = drv_allocator;

	return 0;
}

static const struct virtio_dev_id vtrng_dev_id[] = {
	{VIRTIO_ID_RNG},
	{VIRTIO_ID_INVALID} /* List Terminator */
};

static struct virtio_driver vtrng_drv = {
	.dev_ids = vtrng_dev_id,
	.init    = virtio_rng_drv_init,
	.add_dev = virtio_rng_add_dev,
};
VIRTIO_BUS_REGISTER_DRIVER(&vtrng_drv);
