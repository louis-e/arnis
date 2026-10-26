execute store success score #hit arnis.ds run fill ~-4 ~-26 ~-4 ~4 ~2 ~4 minecraft:air replace #arnis:shatters
execute if score #hit arnis.ds matches 1 run playsound minecraft:block.glass.break block @a[distance=..48] ~ ~-6 ~ 2 0.8
execute if score #hit arnis.ds matches 1 run particle minecraft:block{block_state:"minecraft:glass"} ~ ~-6 ~ 3 6 3 0 40
