#!/usr/bin/env python3
"""
Analyze the problematic polygon to understand why scanline fill might fail.
"""

import matplotlib.pyplot as plt
import matplotlib.patches as mpatches
from matplotlib.path import Path
import numpy as np

# The problematic polygon from way_id=1436361378 (integer coords)
polygon_coords = [
    (0, 114),
    (1, 114),
    (11, 114),
    (75, 115),
    (82, 101),
    (81, 95),
    (68, 94),
    (53, 94),
    (32, 102),
    (25, 55),
    (28, 47),
    (97, 41),
    (230, 21),
    (271, 24),
    (273, 14),
    (271, 3),
    (271, 0),  # [16]
    (52, 0),   # [17]
    (43, 2),   # [18]
    (29, 4),   # [19]
    (30, 0),   # [20]
    (0, 0),    # [21]
]

# Close the polygon
polygon_closed = polygon_coords + [polygon_coords[0]]

def signed_area(coords):
    """Calculate signed area using shoelace formula."""
    area = 0.0
    n = len(coords)
    for i in range(n):
        j = (i + 1) % n
        area += (coords[j][0] - coords[i][0]) * (coords[j][1] + coords[i][1])
    return area / 2.0

def scanline_fill(polygon_coords, verbose=False):
    """
    Simplified scanline fill algorithm - fills from leftmost to rightmost intersection.
    This handles self-intersecting polygons from Sutherland-Hodgman clipping.
    """
    n = len(polygon_coords)
    
    # Calculate bounding box
    min_x = min(c[0] for c in polygon_coords)
    max_x = max(c[0] for c in polygon_coords)
    min_z = min(c[1] for c in polygon_coords)
    max_z = max(c[1] for c in polygon_coords)
    
    print(f"Bounding box: x=[{min_x}, {max_x}], z=[{min_z}, {max_z}]")
    
    filled_area = []
    
    # Analyze specific scanlines
    test_scanlines = [0, 1, 2, 3, 4, 5, 50, 100]
    
    for z in range(min_z, max_z + 1):
        zf = z + 0.5  # Center of pixel
        
        leftmost = None
        rightmost = None
        
        for i in range(n):
            x1, z1 = polygon_coords[i]
            x2, z2 = polygon_coords[(i + 1) % n]
            
            z1f, z2f = float(z1), float(z2)
            
            # Handle horizontal edges
            if z1 == z2:
                if z1 == z:
                    x_min = min(x1, x2)
                    x_max = max(x1, x2)
                    leftmost = min(leftmost, x_min) if leftmost is not None else x_min
                    rightmost = max(rightmost, x_max) if rightmost is not None else x_max
                continue
            
            z_min, z_max = (z1f, z2f) if z1f < z2f else (z2f, z1f)
            
            if zf > z_min and zf <= z_max:
                t = (zf - z1f) / (z2f - z1f)
                x_intersect = x1 + t * (x2 - x1)
                
                leftmost = min(leftmost, x_intersect) if leftmost is not None else x_intersect
                rightmost = max(rightmost, x_intersect) if rightmost is not None else x_intersect
        
        if leftmost is not None and rightmost is not None:
            x_start = int(np.ceil(leftmost))
            x_end = int(np.floor(rightmost))
            
            for x in range(x_start, x_end + 1):
                filled_area.append((x, z))
            
            if z in test_scanlines and verbose:
                print(f"Scanline z={z}: fill x=[{x_start}, {x_end}] (leftmost={leftmost:.2f}, rightmost={rightmost:.2f})")
    
    return filled_area

def visualize_polygon_and_fill():
    """Visualize the polygon and its fill."""
    fig, axes = plt.subplots(1, 3, figsize=(18, 6))
    
    # Plot 1: Just the polygon outline
    ax1 = axes[0]
    xs = [c[0] for c in polygon_closed]
    zs = [c[1] for c in polygon_closed]
    ax1.plot(xs, zs, 'b-', linewidth=2)
    
    # Mark vertices with numbers
    for i, (x, z) in enumerate(polygon_coords):
        ax1.plot(x, z, 'ro', markersize=5)
        ax1.annotate(str(i), (x, z), textcoords="offset points", xytext=(5,5), fontsize=8)
    
    ax1.set_title('Polygon Outline\n(way_id=1436361378)')
    ax1.set_xlabel('X')
    ax1.set_ylabel('Z')
    ax1.grid(True)
    ax1.set_aspect('equal')
    
    # Plot 2: Zoom in on the bottom edge (the problematic area)
    ax2 = axes[1]
    ax2.plot(xs, zs, 'b-', linewidth=2)
    
    # Highlight the "notch" region (vertices 16-21)
    notch_coords = polygon_coords[16:22] + [polygon_coords[0]]
    notch_xs = [c[0] for c in notch_coords]
    notch_zs = [c[1] for c in notch_coords]
    ax2.plot(notch_xs, notch_zs, 'r-', linewidth=3, label='Notch region')
    
    for i in range(16, 22):
        x, z = polygon_coords[i]
        ax2.plot(x, z, 'go', markersize=8)
        ax2.annotate(f'{i}:({x},{z})', (x, z), textcoords="offset points", xytext=(0,10), fontsize=9)
    
    ax2.set_xlim(-10, 300)
    ax2.set_ylim(-5, 30)
    ax2.set_title('Zoomed: Bottom Edge "Notch"\nPoints 16-21 show the self-crossing')
    ax2.set_xlabel('X')
    ax2.set_ylabel('Z')
    ax2.grid(True)
    ax2.legend()
    
    # Plot 3: Scanline fill result
    ax3 = axes[2]
    
    print("=== Analyzing Scanline Fill ===")
    print(f"Polygon signed area: {signed_area(polygon_coords):.2f}")
    
    filled = scanline_fill(polygon_coords, verbose=True)
    print(f"\nTotal filled points: {len(filled)}")
    
    if filled:
        fill_xs = [p[0] for p in filled]
        fill_zs = [p[1] for p in filled]
        ax3.scatter(fill_xs, fill_zs, c='lightblue', s=1, alpha=0.5, label='Filled area')
    
    ax3.plot(xs, zs, 'b-', linewidth=1)
    ax3.set_title(f'Scanline Fill Result\n({len(filled)} points filled)')
    ax3.set_xlabel('X')
    ax3.set_ylabel('Z')
    ax3.grid(True)
    ax3.set_aspect('equal')
    
    plt.tight_layout()
    plt.savefig('polygon_analysis.png', dpi=150)
    print("\nSaved visualization to polygon_analysis.png")
    plt.show()

def check_edge_crossings():
    """Check which edges cross z=0, z=1, etc."""
    print("\n=== Edge Analysis ===")
    n = len(polygon_coords)
    
    for i in range(n):
        x1, z1 = polygon_coords[i]
        x2, z2 = polygon_coords[(i + 1) % n]
        
        print(f"Edge {i}: ({x1},{z1}) -> ({x2},{z2})", end="")
        if z1 == z2:
            print(f"  [HORIZONTAL at z={z1}]")
        else:
            # What z values does this edge cross?
            z_min, z_max = min(z1, z2), max(z1, z2)
            if z_max <= 5:
                print(f"  [crosses z in [{z_min}, {z_max}]]")
            else:
                print()

def trace_bottom_scanlines():
    """Trace exactly what happens at z=0, z=1, z=2, z=3, z=4"""
    print("\n=== Detailed Trace of Bottom Scanlines ===")
    n = len(polygon_coords)
    
    for z in range(6):
        zf = z + 0.5
        print(f"\n--- Scanline z={z} (checking at zf={zf}) ---")
        
        intersections = []
        
        for i in range(n):
            x1, z1 = polygon_coords[i]
            x2, z2 = polygon_coords[(i + 1) % n]
            
            z1f, z2f = float(z1), float(z2)
            
            if z1 == z2:
                if z1 == z:
                    print(f"  Edge {i}: ({x1},{z1})->({x2},{z2}) HORIZONTAL - skipped but ON scanline")
                continue
            
            z_min, z_max = (z1f, z2f) if z1f < z2f else (z2f, z1f)
            
            if zf > z_min and zf <= z_max:
                t = (zf - z1f) / (z2f - z1f)
                x_intersect = x1 + t * (x2 - x1)
                intersections.append(x_intersect)
                print(f"  Edge {i}: ({x1},{z1})->({x2},{z2}) INTERSECTS at x={x_intersect:.2f}")
            else:
                if z_min <= 5 and z_max >= 0:
                    print(f"  Edge {i}: ({x1},{z1})->({x2},{z2}) no intersect (z_range=[{z_min},{z_max}], zf={zf})")
        
        intersections.sort()
        print(f"  Sorted intersections: {[f'{x:.2f}' for x in intersections]}")
        
        # Fill between pairs
        filled_ranges = []
        i = 0
        while i + 1 < len(intersections):
            x_start = int(np.ceil(intersections[i]))
            x_end = int(np.floor(intersections[i + 1]))
            if x_start <= x_end:
                filled_ranges.append((x_start, x_end))
            i += 2
        
        print(f"  Filled ranges: {filled_ranges}")
        total_filled = sum(e - s + 1 for s, e in filled_ranges)
        print(f"  Total pixels on this scanline: {total_filled}")

if __name__ == "__main__":
    check_edge_crossings()
    trace_bottom_scanlines()
    visualize_polygon_and_fill()
