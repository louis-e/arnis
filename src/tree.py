from .main import (
    setBlock,
    fillBlocks,
    oak_leaves,
    oak_log,
    spruce_log,
    birch_leaves,
    birch_log,
)


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


def createTree(x, z, typetree=1):
    if typetree == 1:  # Oak tree
        fillBlocks(oak_log, x, 2, z, x, 10, z)
        fillBlocks(oak_leaves, x - 1, 5, z, x - 1, 11, z)
        fillBlocks(oak_leaves, x + 1, 5, z, x + 1, 11, z)
        fillBlocks(oak_leaves, x, 5, z - 1, x, 11, z - 1)
        fillBlocks(oak_leaves, x, 5, z + 1, x, 11, z + 1)
        fillBlocks(oak_leaves, x, 11, z, x, 12, z)
        round1(oak_leaves, x, 10, z)
        round1(oak_leaves, x, 9, z)
        round1(oak_leaves, x, 8, z)
        round1(oak_leaves, x, 7, z)
        round1(oak_leaves, x, 6, z)
        round1(oak_leaves, x, 5, z)
        round2(oak_leaves, x, 9, z)
        round2(oak_leaves, x, 8, z)
        round2(oak_leaves, x, 7, z)
        round2(oak_leaves, x, 6, z)
        round3(oak_leaves, x, 8, z)
        round3(oak_leaves, x, 7, z)

    elif typetree == 2:  # Spruce tree
        fillBlocks(spruce_log, x, 2, z, x, 11, z)
        fillBlocks(birch_leaves, x - 1, 5, z, x - 1, 12, z)
        fillBlocks(birch_leaves, x + 1, 5, z, x + 1, 12, z)
        fillBlocks(birch_leaves, x, 5, z - 1, x, 12, z - 1)
        fillBlocks(birch_leaves, x, 5, z + 1, x, 12, z + 1)
        setBlock(birch_leaves, x, 12, z)
        round1(birch_leaves, x, 11, z)
        round1(birch_leaves, x, 9, z)
        round1(birch_leaves, x, 8, z)
        round1(birch_leaves, x, 6, z)
        round1(birch_leaves, x, 5, z)
        round2(birch_leaves, x, 8, z)
        round2(birch_leaves, x, 5, z)

    elif typetree == 3:  # Birch tree
        fillBlocks(birch_log, x, 2, z, x, 8, z)
        fillBlocks(birch_leaves, x - 1, 4, z, x - 1, 9, z)
        fillBlocks(birch_leaves, x + 1, 4, z, x + 1, 9, z)
        fillBlocks(birch_leaves, x, 4, z - 1, x, 9, z - 1)
        fillBlocks(birch_leaves, x, 4, z + 1, x, 9, z + 1)
        fillBlocks(birch_leaves, x, 9, z, x, 10, z)
        round1(birch_leaves, x, 8, z)
        round1(birch_leaves, x, 7, z)
        round1(birch_leaves, x, 6, z)
        round1(birch_leaves, x, 5, z)
        round1(birch_leaves, x, 4, z)
        round2(birch_leaves, x, 4, z)
        round2(birch_leaves, x, 5, z)
        round2(birch_leaves, x, 6, z)
