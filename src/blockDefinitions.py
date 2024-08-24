import anvil

air = anvil.Block("minecraft", "air")
birch_leaves = anvil.Block("minecraft", "birch_leaves")
birch_log = anvil.Block("minecraft", "birch_log")
black_concrete = anvil.Block("minecraft", "black_concrete")
blue_flower = anvil.Block("minecraft", "blue_orchid")
brick = anvil.Block("minecraft", "bricks")
carrots = anvil.Block("minecraft", "carrots", {"age": 7})
cauldron = anvil.Block("minecraft", "cauldron")
cobblestone = anvil.Block("minecraft", "cobblestone")
cobblestone_wall = anvil.Block("minecraft", "cobblestone_wall")
dark_oak_door_lower = anvil.Block("minecraft", "dark_oak_door", {"half": "lower"})
dark_oak_door_upper = anvil.Block("minecraft", "dark_oak_door", {"half": "upper"})
dirt = anvil.Block("minecraft", "dirt")
farmland = anvil.Block("minecraft", "farmland")
glass = anvil.Block("minecraft", "glass_pane")
glowstone = anvil.Block("minecraft", "glowstone")
grass = anvil.Block("minecraft", "grass_block")
grass_block = anvil.Block("minecraft", "grass_block")
gravel = anvil.Block("minecraft", "gravel")
gray_concrete = anvil.Block("minecraft", "gray_concrete")
green_stained_hardened_clay = anvil.Block("minecraft", "green_terracotta")
hay_bale = anvil.Block("minecraft", "hay_block")
iron_block = anvil.Block("minecraft", "iron_block")
light_gray_concrete = anvil.Block("minecraft", "light_gray_concrete")
oak_fence = anvil.Block("minecraft", "oak_fence")
oak_leaves = anvil.Block("minecraft", "oak_leaves")
oak_log = anvil.Block("minecraft", "oak_log")
oak_planks = anvil.Block("minecraft", "oak_planks")
podzol = anvil.Block("minecraft", "podzol")
potatoes = anvil.Block("minecraft", "potatoes", {"age": 7})
rail = anvil.Block("minecraft", "rail")
red_flower = anvil.Block("minecraft", "poppy")
sand = anvil.Block("minecraft", "sand")
scaffolding = anvil.Block("minecraft", "scaffolding")
sponge = anvil.Block("minecraft", "sponge")
spruce_log = anvil.Block("minecraft", "spruce_log")
stone = anvil.Block("minecraft", "stone")
stone_block_slab = anvil.Block("minecraft", "stone_slab")
stone_brick_slab = anvil.Block("minecraft", "stone_brick_slab")
water = anvil.Block("minecraft", "water")
wheat = anvil.Block("minecraft", "wheat", {"age": 7})
white_concrete = anvil.Block("minecraft", "white_concrete")
white_flower = anvil.Block("minecraft", "azure_bluet")
white_stained_glass = anvil.Block("minecraft", "white_stained_glass")
yellow_flower = anvil.Block("minecraft", "dandelion")

# Variations for building corners
building_corner_variations = [
    anvil.Block("minecraft", "stone_bricks"),
    anvil.Block("minecraft", "cobblestone"),
    anvil.Block("minecraft", "bricks"),
    anvil.Block("minecraft", "mossy_cobblestone"),
    anvil.Block("minecraft", "sandstone"),
    anvil.Block("minecraft", "red_nether_bricks"),
    anvil.Block("minecraft", "blackstone"),
    anvil.Block("minecraft", "smooth_quartz"),
    anvil.Block("minecraft", "chiseled_stone_bricks"),
    anvil.Block("minecraft", "polished_basalt"),
    anvil.Block("minecraft", "cut_sandstone"),
    anvil.Block("minecraft", "polished_blackstone_bricks"),
]

# Variations for building walls
building_wall_variations = [
    anvil.Block("minecraft", "white_terracotta"),
    anvil.Block("minecraft", "gray_terracotta"),
    anvil.Block("minecraft", "bricks"),
    anvil.Block("minecraft", "smooth_sandstone"),
    anvil.Block("minecraft", "red_terracotta"),
    anvil.Block("minecraft", "polished_diorite"),
    anvil.Block("minecraft", "smooth_stone"),
    anvil.Block("minecraft", "polished_andesite"),
    anvil.Block("minecraft", "warped_planks"),
    anvil.Block("minecraft", "end_stone_bricks"),
    anvil.Block("minecraft", "smooth_red_sandstone"),
    anvil.Block("minecraft", "nether_bricks"),
]

# Variations for building floors
building_floor_variations = [
    anvil.Block("minecraft", "oak_planks"),
    anvil.Block("minecraft", "spruce_planks"),
    anvil.Block("minecraft", "dark_oak_planks"),
    anvil.Block("minecraft", "stone_bricks"),
    anvil.Block("minecraft", "polished_granite"),
    anvil.Block("minecraft", "polished_diorite"),
    anvil.Block("minecraft", "acacia_planks"),
    anvil.Block("minecraft", "jungle_planks"),
    anvil.Block("minecraft", "warped_planks"),
    anvil.Block("minecraft", "purpur_block"),
    anvil.Block("minecraft", "smooth_red_sandstone"),
    anvil.Block("minecraft", "polished_blackstone"),
]
