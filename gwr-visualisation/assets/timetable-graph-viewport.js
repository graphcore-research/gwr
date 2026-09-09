// Copyright (c) 2026 Graphcore Ltd. All rights reserved.

(() => {
  const App = window.GWR_VISUALISATION_APP;

  class ViewportTransform {
    constructor(x = 0, y = 0, k = 1) {
      this.x = x;
      this.y = y;
      this.k = k;
    }

    invert([x, y]) {
      return [(x - this.x) / this.k, (y - this.y) / this.k];
    }

    apply([x, y]) {
      return [x * this.k + this.x, y * this.k + this.y];
    }
  }

  function createViewportTransform(x = 0, y = 0, k = 1) {
    return new ViewportTransform(x, y, k);
  }

  function pointerPosition(event, element) {
    const bounds = element.getBoundingClientRect();
    return [event.clientX - bounds.left, event.clientY - bounds.top];
  }

  function createCanvasViewport(
    canvas,
    { minScale = 0.005, maxScale = 8, onChange },
  ) {
    let transform = createViewportTransform();
    let animationFrame = null;
    let gesture = null;
    let suppressClick = false;
    const pointers = new Map();

    function clampScale(scale) {
      return Math.max(minScale, Math.min(maxScale, scale));
    }

    function update(next) {
      transform = createViewportTransform(
        Number.isFinite(next.x) ? next.x : transform.x,
        Number.isFinite(next.y) ? next.y : transform.y,
        clampScale(Number.isFinite(next.k) ? next.k : transform.k),
      );
      canvas.__zoom = transform;
      onChange(transform);
    }

    function cancelAnimation() {
      if (animationFrame !== null) {
        cancelAnimationFrame(animationFrame);
        animationFrame = null;
      }
    }

    function setTransform(next, { animate = false, duration = 250 } = {}) {
      cancelAnimation();
      const target = createViewportTransform(
        next.x,
        next.y,
        clampScale(next.k),
      );
      if (!animate || duration <= 0) {
        update(target);
        return;
      }
      const start = transform;
      const started = performance.now();
      const scaleRatio = target.k / start.k;
      const step = (now) => {
        const progress = Math.min(1, (now - started) / duration);
        const eased = 1 - (1 - progress) ** 3;
        update(
          createViewportTransform(
            start.x + (target.x - start.x) * eased,
            start.y + (target.y - start.y) * eased,
            start.k * scaleRatio ** eased,
          ),
        );
        if (progress < 1) {
          animationFrame = requestAnimationFrame(step);
        } else {
          animationFrame = null;
        }
      };
      animationFrame = requestAnimationFrame(step);
    }

    function scaleBy(factor, { animate = true, duration = 160, point } = {}) {
      const anchor = point || [canvas.clientWidth / 2, canvas.clientHeight / 2];
      const graphPoint = transform.invert(anchor);
      const scale = clampScale(transform.k * factor);
      setTransform(
        createViewportTransform(
          anchor[0] - graphPoint[0] * scale,
          anchor[1] - graphPoint[1] * scale,
          scale,
        ),
        { animate, duration },
      );
    }

    function beginGesture() {
      const positions = [...pointers.values()];
      if (positions.length === 1) {
        gesture = {
          kind: "pan",
          start: positions[0],
          transform,
          moved: false,
        };
        return;
      }
      const [first, second] = positions;
      const midpoint = [(first[0] + second[0]) / 2, (first[1] + second[1]) / 2];
      gesture = {
        kind: "pinch",
        distance: Math.hypot(second[0] - first[0], second[1] - first[1]),
        graphPoint: transform.invert(midpoint),
        transform,
        moved: false,
      };
    }

    function moveGesture() {
      const positions = [...pointers.values()];
      if (!gesture || !positions.length) return;
      if (gesture.kind === "pan" && positions.length === 1) {
        const dx = positions[0][0] - gesture.start[0];
        const dy = positions[0][1] - gesture.start[1];
        gesture.moved ||= Math.hypot(dx, dy) > 3;
        update(
          createViewportTransform(
            gesture.transform.x + dx,
            gesture.transform.y + dy,
            gesture.transform.k,
          ),
        );
        return;
      }
      if (gesture.kind !== "pinch" || positions.length < 2) return;
      const [first, second] = positions;
      const midpoint = [(first[0] + second[0]) / 2, (first[1] + second[1]) / 2];
      const distance = Math.hypot(second[0] - first[0], second[1] - first[1]);
      const scale = clampScale(
        (gesture.transform.k * distance) / Math.max(gesture.distance, 1),
      );
      gesture.moved ||= Math.abs(distance - gesture.distance) > 3;
      update(
        createViewportTransform(
          midpoint[0] - gesture.graphPoint[0] * scale,
          midpoint[1] - gesture.graphPoint[1] * scale,
          scale,
        ),
      );
    }

    canvas.addEventListener(
      "wheel",
      (event) => {
        event.preventDefault();
        cancelAnimation();
        const delta =
          event.deltaY *
          (event.deltaMode === 1
            ? 16
            : event.deltaMode === 2
              ? canvas.clientHeight
              : 1);
        const sensitivity = event.ctrlKey ? 0.02 : 0.002;
        const exponent = Math.max(-1, Math.min(1, -delta * sensitivity));
        scaleBy(2 ** exponent, {
          animate: false,
          point: pointerPosition(event, canvas),
        });
      },
      { passive: false },
    );
    canvas.addEventListener("pointerdown", (event) => {
      if (event.button !== 0) return;
      event.preventDefault();
      cancelAnimation();
      canvas.setPointerCapture(event.pointerId);
      pointers.set(event.pointerId, pointerPosition(event, canvas));
      beginGesture();
    });
    canvas.addEventListener("pointermove", (event) => {
      if (!pointers.has(event.pointerId)) return;
      pointers.set(event.pointerId, pointerPosition(event, canvas));
      moveGesture();
    });
    const finishPointer = (event) => {
      if (!pointers.has(event.pointerId)) return;
      suppressClick ||= gesture?.moved === true;
      pointers.delete(event.pointerId);
      if (canvas.hasPointerCapture(event.pointerId)) {
        canvas.releasePointerCapture(event.pointerId);
      }
      gesture = null;
      if (pointers.size) beginGesture();
    };
    canvas.addEventListener("pointerup", finishPointer);
    canvas.addEventListener("pointercancel", finishPointer);
    canvas.addEventListener(
      "click",
      (event) => {
        if (!suppressClick) return;
        suppressClick = false;
        event.preventDefault();
        event.stopImmediatePropagation();
      },
      true,
    );
    update(transform);

    return {
      cancelAnimation,
      getTransform: () => transform,
      scaleBy,
      scaleExtent: () => [minScale, maxScale],
      setTransform,
    };
  }

  Object.assign(App, {
    createCanvasViewport,
    createViewportTransform,
    pointerPosition,
  });
})();
