from .processData import (
    setBlock,
    fillBlocks,
)
from .blockDefinitions import *


def round1(material, x, y, z):
    setBlock(material, x - 2, y, z)
    setBlock(material, x + 2, y, z)
    setBlock(material, x, y, z - 2)
    setBlock(material, x, y, z + 2)
    setBlock(material, x - 1, y, z - 1)
    setBlock(material, x + 1, y, z + 1)
    setBlock(material, x + 1, y, z - 1)
    setBlock(material, x - 1, y, z + 1)


def round2(material, x, y, z):
    setBlock(material, x + 3, y, z)
    setBlock(material, x + 2, y, z - 1)
    setBlock(material, x + 2, y, z + 1)
    setBlock(material, x + 1, y, z - 2)
    setBlock(material, x + 1, y, z + 2)
    setBlock(material, x - 3, y, z)
    setBlock(material, x - 2, y, z - 1)
    setBlock(material, x - 2, y, z + 1)
    setBlock(material, x - 1, y, z + 2)
    setBlock(material, x - 1, y, z - 2)
    setBlock(material, x, y, z - 3)
    setBlock(material, x, y, z + 3)


def round3(material, x, y, z):
    setBlock(material, x + 3, y, z - 1)
    setBlock(material, x + 3, y, z + 1)
    setBlock(material, x + 2, y, z - 2)
    setBlock(material, x + 2, y, z + 2)
    setBlock(material, x + 1, y, z - 3)
    setBlock(material, x + 1, y, z + 3)
    setBlock(material, x - 3, y, z - 1)
    setBlock(material, x - 3, y, z + 1)
    setBlock(material, x - 2, y, z - 2)
    setBlock(material, x - 2, y, z + 2)
    setBlock(material, x - 1, y, z + 3)
    setBlock(material, x - 1, y, z - 3)


def createTree(x, y, z, typetree=1):
    if typetree == 1:  # Oak tree
        fillBlocks(oak_log, x, y, z, x, y + 8, z)
        fillBlocks(oak_leaves, x - 1, y + 3, z, x - 1, y + 9, z)
        fillBlocks(oak_leaves, x + 1, y + 3, z, x + 1, y + 9, z)
        fillBlocks(oak_leaves, x, y + 3, z - 1, x, y + 9, z - 1)
        fillBlocks(oak_leaves, x, y + 3, z + 1, x, y + 9, z + 1)
        fillBlocks(oak_leaves, x, y + 9, z, x, y + 10, z)
        round1(oak_leaves, x, y + 8, z)
        round1(oak_leaves, x, y + 7, z)
        round1(oak_leaves, x, y + 6, z)
        round1(oak_leaves, x, y + 5, z)
        round1(oak_leaves, x, y + 4, z)
        round1(oak_leaves, x, y + 3, z)
        round2(oak_leaves, x, y + 7, z)
        round2(oak_leaves, x, y + 6, z)
        round2(oak_leaves, x, y + 5, z)
        round2(oak_leaves, x, y + 4, z)
        round3(oak_leaves, x, y + 6, z)
        round3(oak_leaves, x, y + 5, z)

    elif typetree == 2:  # Spruce tree
        fillBlocks(spruce_log, x, y, z, x, y + 9, z)
        fillBlocks(birch_leaves, x - 1, y + 3, z, x - 1, y + 10, z)
        fillBlocks(birch_leaves, x + 1, y + 3, z, x + 1, y + 10, z)
        fillBlocks(birch_leaves, x, y + 3, z - 1, x, y + 10, z - 1)
        fillBlocks(birch_leaves, x, y + 3, z + 1, x, y + 10, z + 1)
        setBlock(birch_leaves, x, y + 10, z)
        round1(birch_leaves, x, y + 9, z)
        round1(birch_leaves, x, y + 7, z)
        round1(birch_leaves, x, y + 6, z)
        round1(birch_leaves, x, y + 4, z)
        round1(birch_leaves, x, y + 3, z)
        round2(birch_leaves, x, y + 6, z)
        round2(birch_leaves, x, y + 3, z)

    elif typetree == 3:  # Birch tree
        fillBlocks(birch_log, x, y, z, x, y + 6, z)
        fillBlocks(birch_leaves, x - 1, y + 2, z, x - 1, y + 7, z)
        fillBlocks(birch_leaves, x + 1, y + 2, z, x + 1, y + 7, z)
        fillBlocks(birch_leaves, x, y + 2, z - 1, x, y + 7, z - 1)
        fillBlocks(birch_leaves, x, y + 2, z + 1, x, y + 7, z + 1)
        fillBlocks(birch_leaves, x, y + 7, z, x, y + 8, z)
        round1(birch_leaves, x, y + 6, z)
        round1(birch_leaves, x, y + 5, z)
        round1(birch_leaves, x, y + 4, z)
        round1(birch_leaves, x, y + 3, z)
        round1(birch_leaves, x, y + 2, z)
        round2(birch_leaves, x, y + 2, z)
        round2(birch_leaves, x, y + 3, z)
        round2(birch_leaves, x, y + 4, z)
