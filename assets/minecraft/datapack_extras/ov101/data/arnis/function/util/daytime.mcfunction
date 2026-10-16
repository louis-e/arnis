# 26.1 moved time of day onto world clocks; the overworld clock still wraps every 24000 ticks.
execute store result score #daytime arnis run time query time
scoreboard players operation #daytime arnis %= #24000 arnis
