#!/usr/bin/env python3
"""Analyze performance data from Windows Performance Monitor CSV exports."""

import csv
from datetime import datetime
from pathlib import Path

def parse_pdh_csv(filepath):
    """Parse a Windows Performance Monitor CSV file."""
    data = []
    
    with open(filepath, 'r', encoding='utf-8-sig', errors='replace') as f:
        reader = csv.reader(f)
        header = next(reader)
        
        # Clean up column names - extract the metric name
        clean_cols = []
        for col in header:
            if 'Verfügbare MB' in col or 'Verf' in col:
                clean_cols.append('available_mb')
            elif 'Zugesicherte' in col:
                clean_cols.append('committed_pct')
            elif 'Bytes geschrieben' in col:
                clean_cols.append('disk_write_bytes_sec')
            elif 'Arbeitsseiten' in col and 'arnis-windows' not in col:
                clean_cols.append('working_set')
            elif 'Arbeitsseiten' in col and 'arnis-windows' in col:
                clean_cols.append('gui_working_set')
            elif 'Private Bytes' in col and 'arnis-windows' not in col:
                clean_cols.append('private_bytes')
            elif 'Private Bytes' in col and 'arnis-windows' in col:
                clean_cols.append('gui_private_bytes')
            elif 'Prozessorzeit' in col and 'arnis-windows' not in col and 'Prozessorinformationen' not in col:
                clean_cols.append('cpu_pct')
            elif 'Prozessorzeit' in col and 'arnis-windows' in col:
                clean_cols.append('gui_cpu_pct')
            elif 'Threadanzahl' in col and 'arnis-windows' not in col:
                clean_cols.append('thread_count')
            elif 'Threadanzahl' in col and 'arnis-windows' in col:
                clean_cols.append('gui_thread_count')
            elif 'PDH-CSV' in col:
                clean_cols.append('timestamp')
            else:
                clean_cols.append(col[:30])  # truncate long names
        
        for row in reader:
            if not row or not row[0].strip():
                continue
            entry = {}
            for i, val in enumerate(row):
                if i >= len(clean_cols):
                    break
                col_name = clean_cols[i]
                if col_name == 'timestamp':
                    try:
                        entry[col_name] = datetime.strptime(val.strip(), '%m/%d/%Y %H:%M:%S.%f')
                    except:
                        entry[col_name] = val
                elif val.strip() == '' or val.strip() == ' ':
                    entry[col_name] = None
                else:
                    try:
                        entry[col_name] = float(val)
                    except:
                        entry[col_name] = val
            data.append(entry)
    
    return data


def analyze_run(data, name):
    """Analyze a single run's data."""
    print(f"\n{'='*60}")
    print(f"  {name}")
    print(f"{'='*60}")
    
    # Time range
    timestamps = [d.get('timestamp') for d in data if isinstance(d.get('timestamp'), datetime)]
    if timestamps:
        duration = (timestamps[-1] - timestamps[0]).total_seconds()
        print(f"Duration: {duration:.1f}s ({duration/60:.1f} min)")
    
    # Memory usage (working set) - prefer 'working_set' (arnis backend) over gui_working_set
    working_sets = [d.get('working_set') for d in data if d.get('working_set') is not None]
    gui_ws = [d.get('gui_working_set') for d in data if d.get('gui_working_set') is not None]
    
    # Use GUI working set if backend working set not available (before scenario)
    if working_sets:
        max_ws = max(working_sets) / (1024**3)  # GB
        avg_ws = sum(working_sets) / len(working_sets) / (1024**3)
        print(f"Backend Working Set: max={max_ws:.2f} GB, avg={avg_ws:.2f} GB")
    
    if gui_ws:
        max_gui_ws = max(gui_ws) / (1024**3)
        print(f"GUI Working Set: max={max_gui_ws:.2f} GB")
        # For before, we only have GUI data, so use that as the main metric
        if not working_sets:
            working_sets = gui_ws
            max_ws = max_gui_ws
    
    # Private bytes
    private = [d.get('private_bytes') for d in data if d.get('private_bytes') is not None]
    if private:
        max_private = max(private) / (1024**3)
        avg_private = sum(private) / len(private) / (1024**3)
        print(f"Private Bytes: max={max_private:.2f} GB, avg={avg_private:.2f} GB")
    
    # Available system memory
    avail = [d.get('available_mb') for d in data if d.get('available_mb') is not None]
    if avail:
        min_avail = min(avail) / 1024  # GB
        max_avail = max(avail) / 1024
        print(f"System Available Memory: min={min_avail:.2f} GB, max={max_avail:.2f} GB")
    
    # CPU usage
    cpu = [d.get('cpu_pct') for d in data if d.get('cpu_pct') is not None]
    if cpu:
        max_cpu = max(cpu)
        avg_cpu = sum(cpu) / len(cpu)
        print(f"CPU %: max={max_cpu:.1f}%, avg={avg_cpu:.1f}%")
    
    # Thread count
    threads = [d.get('thread_count') for d in data if d.get('thread_count') is not None]
    if threads:
        max_threads = max(threads)
        print(f"Thread count: max={int(max_threads)}")
    
    # Disk writes
    disk = [d.get('disk_write_bytes_sec') for d in data if d.get('disk_write_bytes_sec') is not None]
    if disk:
        max_disk = max(disk) / (1024**2)  # MB/s
        avg_disk = sum(disk) / len(disk) / (1024**2)
        print(f"Disk Write: max={max_disk:.1f} MB/s, avg={avg_disk:.1f} MB/s")
    
    return {
        'duration': duration if timestamps else 0,
        'max_working_set_gb': max(working_sets) / (1024**3) if working_sets else 0,
        'max_private_bytes_gb': max(private) / (1024**3) if private else 0,
        'avg_cpu': sum(cpu) / len(cpu) if cpu else 0,
        'max_cpu': max(cpu) if cpu else 0,
    }


def main():
    print("Performance Analysis: BEFORE vs AFTER Parallel Processing")
    print("=" * 60)
    
    before_path = Path("arnis_before.csv")
    after_path = Path("arnis_after.csv")
    
    if before_path.exists():
        before_data = parse_pdh_csv(before_path)
        before_stats = analyze_run(before_data, "BEFORE (Sequential)")
    else:
        print("arnis_before.csv not found")
        before_stats = None
    
    if after_path.exists():
        after_data = parse_pdh_csv(after_path)
        after_stats = analyze_run(after_data, "AFTER (Parallel)")
    else:
        print("arnis_after.csv not found")
        after_stats = None
    
    # Comparison
    if before_stats and after_stats:
        print(f"\n{'='*60}")
        print("  COMPARISON")
        print(f"{'='*60}")
        
        time_diff = after_stats['duration'] - before_stats['duration']
        time_ratio = after_stats['duration'] / before_stats['duration'] if before_stats['duration'] > 0 else 0
        print(f"Duration: {before_stats['duration']:.1f}s -> {after_stats['duration']:.1f}s ({time_ratio:.2f}x, {time_diff:+.1f}s)")
        
        mem_ratio = after_stats['max_working_set_gb'] / before_stats['max_working_set_gb'] if before_stats['max_working_set_gb'] > 0 else 0
        print(f"Peak Memory: {before_stats['max_working_set_gb']:.2f} GB -> {after_stats['max_working_set_gb']:.2f} GB ({mem_ratio:.2f}x)")
        
        cpu_diff = after_stats['avg_cpu'] - before_stats['avg_cpu']
        print(f"Avg CPU: {before_stats['avg_cpu']:.1f}% -> {after_stats['avg_cpu']:.1f}% ({cpu_diff:+.1f}%)")


if __name__ == '__main__':
    main()
