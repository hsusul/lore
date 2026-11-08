import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { expect, it, vi } from "vitest";

import type { Dot, MapModel } from "../map/layout";
import { MapCanvas } from "./MapCanvas";

const dot = (id: string, area: string, x: number): Dot => ({
  id,
  area,
  label: id,
  item: { kind: "note" } as Dot["item"],
  r: 3,
  x,
  y: 0,
});

const model: MapModel = {
  clusters: [
    { key: "global", name: "Henry", x: 0, y: 0, r: 10, lessons: 0 },
    { key: "skills", name: "Skills", x: 100, y: 0, r: 10, lessons: 0 },
  ],
  dots: [
    dot("note:a", "global", 0),
    dot("skill:review", "skills", 95),
    dot("skill:tdd", "skills", 105),
  ],
  bounds: [-40, -40, 140, 40],
};

it("reaches and opens a dot in every cluster from the keyboard alone", async () => {
  const user = userEvent.setup();
  const onSelect = vi.fn();
  render(
    <MapCanvas
      model={model}
      selectedId={null}
      inset={0}
      onSelect={onSelect}
      onDeselect={() => {}}
      label="Map"
    />,
  );

  await user.tab();
  expect(document.activeElement).toBe(screen.getByRole("img", { name: "Map" }));

  await user.keyboard("{ArrowRight}{Enter}{ArrowRight}{Enter}");
  expect(onSelect).toHaveBeenLastCalledWith(model.dots[2]);

  await user.keyboard("{Escape}{ArrowLeft}{Enter}{Enter}");
  expect(onSelect).toHaveBeenLastCalledWith(model.dots[0]);
});
