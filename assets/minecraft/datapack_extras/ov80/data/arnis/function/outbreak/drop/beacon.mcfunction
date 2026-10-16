particle minecraft:campfire_signal_smoke ~ ~1 ~ 0.15 1.5 0.15 0.01 4 force
execute unless block ~ ~ ~ minecraft:chest run function arnis:outbreak/drop/remove
execute if block ~ ~ ~ minecraft:chest unless data block ~ ~ ~ LootTable run function arnis:outbreak/drop/remove
