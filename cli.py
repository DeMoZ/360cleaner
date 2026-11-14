"""
Command-line interface for 360 photo person removal.
Usage:
    python cli.py photo1.jpg photo2.jpg -o clean_360.jpg [--swap] [--threshold 24]
"""

import argparse
import sys
import os
import core


def main():
    parser = argparse.ArgumentParser(description="Remove photographer / person from two 360 photos without AI.")
    parser.add_argument("photo1", help="Path to first 360 equirectangular photo")
    parser.add_argument("photo2", help="Path to second 360 equirectangular photo")
    parser.add_argument("-o", "--output", default="clean_360.jpg", help="Output 360 image path (default: clean_360.jpg)")
    parser.add_argument("--threshold", type=int, default=24, help="Difference threshold (default: 24)")
    parser.add_argument("--dilate", type=int, default=25, help="Silhouette dilation radius (default: 25)")
    parser.add_argument("--feather", type=int, default=20, help="Feather seam radius (default: 20)")
    parser.add_argument("--swap", action="store_true", help="Swap cluster assignment if needed")

    args = parser.parse_args()

    if not os.path.exists(args.photo1):
        print(f"Error: file not found: {args.photo1}")
        sys.exit(1)
    if not os.path.exists(args.photo2):
        print(f"Error: file not found: {args.photo2}")
        sys.exit(1)

    print(f"Processing 360 photos:")
    print(f"  Photo 1: {args.photo1}")
    print(f"  Photo 2: {args.photo2}")
    print(f"  Threshold: {args.threshold}, Dilate: {args.dilate}, Feather: {args.feather}, Swap: {args.swap}")

    result, mask_A, mask_B, angle_A, angle_B = core.process_clean_plate(
        args.photo1,
        args.photo2,
        output_path=args.output,
        threshold=args.threshold,
        dilate_radius=args.dilate,
        feather_radius=args.feather,
        swap=args.swap
    )

    print(f"Success! Saved clean 360 photo to {args.output}")
    print(f"  Detected person at angle A: {angle_A:.1f}°")
    print(f"  Detected person at angle B: {angle_B:.1f}°")


if __name__ == "__main__":
    main()
